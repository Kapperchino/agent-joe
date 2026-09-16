use super::snapshot::WorktreeSnapshot;
use crate::{git::GitRepository, workspace::WorkspacePolicy};
use anyhow::Context;
use git2::{Oid, RepositoryState, Signature};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, path::PathBuf};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionWorktree {
    id: String,
    pub path: PathBuf,
    pub target: String,
}

pub enum MergeOutcome {
    Unchanged,
    Merged { target: String, commit: String },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MergeConflict {
    pub approved: String,
    pub target: String,
    pub paths: Vec<PathBuf>,
}

impl std::fmt::Display for MergeConflict {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "Merge conflicts with main in {:?}", self.paths)
    }
}

impl std::error::Error for MergeConflict {}

impl MergeConflict {
    fn new(approved: Oid, target: Oid, index: &git2::Index) -> anyhow::Result<Self> {
        let paths = index
            .conflicts()?
            .try_fold(BTreeSet::new(), |paths, conflict| {
                let conflict = conflict?;
                [conflict.ancestor, conflict.our, conflict.their]
                    .into_iter()
                    .flatten()
                    .try_fold(paths, |mut paths, entry| {
                        paths.insert(PathBuf::from(std::str::from_utf8(&entry.path)?));
                        Ok::<_, anyhow::Error>(paths)
                    })
            })?;
        Ok(Self {
            approved: approved.to_string(),
            target: target.to_string(),
            paths: paths.into_iter().collect(),
        })
    }

    fn resolved_snapshot(&self, snapshot: WorktreeSnapshot) -> anyhow::Result<WorktreeSnapshot> {
        self.paths.iter().try_fold(snapshot, |snapshot, path| {
            let unresolved = snapshot
                .files
                .get(path)
                .map(|version| version.text())
                .transpose()?
                .is_some_and(|text| {
                    text.lines().any(|line| {
                        line.starts_with("<<<<<<<")
                            || line.starts_with("|||||||")
                            || line.starts_with("=======")
                            || line.starts_with(">>>>>>>")
                    })
                });
            match unresolved {
                true => Err(anyhow::anyhow!(
                    "Unresolved conflict markers in {}",
                    path.display()
                )),
                false => Ok(snapshot),
            }
        })
    }
}

enum ResolutionState {
    Pending { target: Oid },
    Committed,
}

impl ResolutionState {
    fn new(
        conflict: &MergeConflict,
        parent: &git2::Commit<'_>,
        operation: RepositoryState,
        merge_heads: &[Oid],
    ) -> anyhow::Result<Self> {
        let approved = Oid::from_str(&conflict.approved)?;
        let target = Oid::from_str(&conflict.target)?;
        let committed = parent.parent_count() == 2
            && parent.parent_id(0)? == approved
            && parent.parent_id(1)? == target;
        match operation {
            RepositoryState::Merge if merge_heads != [target] => Err(anyhow::anyhow!(
                "Session conflict resolution targets a different merge"
            )),
            RepositoryState::Merge if parent.id() == approved => Ok(Self::Pending { target }),
            RepositoryState::Clean | RepositoryState::Merge if committed => Ok(Self::Committed),
            _ => Err(anyhow::anyhow!(
                "Session conflict resolution is not prepared or its branch changed"
            )),
        }
    }

    fn target<'repo>(
        &self,
        repo: &'repo git2::Repository,
    ) -> anyhow::Result<Option<git2::Commit<'repo>>> {
        match self {
            Self::Pending { target } => Ok(Some(repo.find_commit(*target)?)),
            Self::Committed => Ok(None),
        }
    }
}

enum MergeState {
    Integrated,
    FastForward,
    Diverged,
}

#[derive(Default)]
struct CommitSummary {
    added: Vec<String>,
    updated: Vec<String>,
    removed: Vec<String>,
}

impl CommitSummary {
    fn new(
        repo: &git2::Repository,
        before: &git2::Tree<'_>,
        after: &git2::Tree<'_>,
    ) -> anyhow::Result<Self> {
        let diff = repo.diff_tree_to_tree(Some(before), Some(after), None)?;
        diff.deltas()
            .try_fold(Self::default(), |mut summary, delta| {
                let path = delta
                    .new_file()
                    .path()
                    .or_else(|| delta.old_file().path())
                    .ok_or_else(|| anyhow::anyhow!("Changed file has no Git path"))?
                    .to_string_lossy()
                    .escape_debug()
                    .to_string();
                match delta.status() {
                    git2::Delta::Added => summary.added.push(path),
                    git2::Delta::Deleted => summary.removed.push(path),
                    _ => summary.updated.push(path),
                }
                Ok(summary)
            })
    }

    fn subject(&self) -> String {
        let detailed = self.describe(true);
        let mut subject = match detailed.chars().count() {
            0 => "Merge session changes already present in main".to_owned(),
            ..=72 => detailed,
            _ => self.describe(false),
        };
        if let Some(first) = subject.get_mut(..1) {
            first.make_ascii_uppercase();
        }
        subject
    }

    fn describe(&self, detailed: bool) -> String {
        [
            Self::describe_paths("add", &self.added, detailed),
            Self::describe_paths("update", &self.updated, detailed),
            Self::describe_paths("remove", &self.removed, detailed),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join("; ")
    }

    fn describe_paths(action: &str, paths: &[String], detailed: bool) -> Option<String> {
        match paths.len() {
            0 => None,
            _ if detailed => Some(format!("{action} {}", paths.join(", "))),
            1 => Some(format!("{action} 1 file")),
            count => Some(format!("{action} {count} files")),
        }
    }
}

struct SessionCleanup<'repo> {
    reference: String,
    worktree: git2::Worktree,
    transaction: git2::Transaction<'repo>,
}

impl<'repo> SessionCleanup<'repo> {
    fn new(
        session: &SessionWorktree,
        project: &WorkspacePolicy,
        git: &'repo GitRepository,
        approved: &str,
    ) -> anyhow::Result<Self> {
        let workspace = session.workspace(project)?;
        let child = GitRepository::required(&workspace)?;
        let reference = format!("refs/heads/{}", session.branch());
        let target_reference = format!("refs/heads/{}", session.target);
        let mut transaction = git.repo.transaction()?;
        transaction.lock_ref(&reference)?;
        transaction.lock_ref(&target_reference)?;
        transaction.lock_ref("HEAD")?;
        let approved_id = Oid::from_str(approved)?;
        let target = git.repo.refname_to_id(&target_reference)?;
        let expected = WorktreeSnapshot::base(git, approved)?;
        let actual = WorktreeSnapshot::for_session_cleanup(&workspace, &child, &expected)?;
        if let Some(entry) = child.status(&workspace)?.entries.first() {
            Err(anyhow::anyhow!(
                "Cleanup conflict: uncommitted change: {}. Preserve or restore this local change before retrying",
                entry.path.display()
            ))?;
        }
        let integrated =
            target == approved_id || git.repo.graph_descendant_of(target, approved_id)?;
        let unchanged = git.repo.refname_to_id(&reference)? == approved_id
            && actual.head == expected.head
            && actual.files == expected.files
            && child.repo.state() == RepositoryState::Clean;
        match integrated && unchanged {
            true => Ok(()),
            false => Err(anyhow::anyhow!(
                "Cleanup conflict: session has unmerged commits, local edits, or a pending Git operation"
            )),
        }?;
        match git.repo.head()?.name()? == reference {
            true => Err(anyhow::anyhow!(
                "Session branch is checked out in the source repository"
            )),
            false => Ok(()),
        }?;
        let worktree = git.repo.find_worktree(&session.id)?;
        let cleanup = match worktree.path() == session.path {
            true => Ok(Self {
                reference,
                worktree,
                transaction,
            }),
            false => Err(anyhow::anyhow!("Session worktree registration changed")),
        }?;
        git.repo
            .worktrees()?
            .iter()
            .try_fold(cleanup, |cleanup, name| {
                let name = name?.ok_or_else(|| anyhow::anyhow!("Worktree name is not UTF-8"))?;
                let checked_out = name != session.id
                    && git2::Repository::open(git.repo.find_worktree(name)?.path())?
                        .head()?
                        .name()?
                        == cleanup.reference;
                match checked_out {
                    true => Err(anyhow::anyhow!(
                        "Session branch is checked out in another worktree"
                    )),
                    false => Ok(cleanup),
                }
            })
    }

    fn execute(mut self) -> anyhow::Result<()> {
        let mut options = git2::WorktreePruneOptions::new();
        options.valid(true).working_tree(true);
        self.worktree.prune(Some(&mut options))?;
        self.transaction.remove(&self.reference)?;
        self.transaction.commit()?;
        Ok(())
    }
}

impl SessionWorktree {
    pub fn create(
        project: &WorkspacePolicy,
        id: &str,
        source: Option<&Self>,
    ) -> anyhow::Result<Option<Self>> {
        match GitRepository::open(project)? {
            None => Ok(None),
            Some(_) => {
                let git = GitRepository::source(project)?;
                let id = uuid::Uuid::parse_str(id)?.to_string();
                let target = match source {
                    Some(source) => source.target.clone(),
                    None => "main".to_owned(),
                };
                let base = match source {
                    Some(source) => source.checkpoint(project)?,
                    None => git
                        .repo
                        .refname_to_id(&format!("refs/heads/{target}"))
                        .context("Session worktrees require a committed main branch")?,
                };
                WorktreeSnapshot::base(&git, &base.to_string())?;
                let worktree = Self {
                    path: project.root().join(".joe-worktrees").join(&id),
                    id,
                    target,
                };
                project.create_parent_dirs(&worktree.path)?;
                let commit = git.repo.find_commit(base)?;
                let branch = git.repo.branch(&worktree.branch(), &commit, false)?;
                let mut options = git2::WorktreeAddOptions::new();
                options.reference(Some(branch.get()));
                git.repo
                    .worktree(&worktree.id, &worktree.path, Some(&options))?;
                worktree.workspace(project)?;
                Ok(Some(worktree))
            }
        }
    }

    fn branch(&self) -> String {
        format!("joe/session/{}", self.id)
    }

    pub fn workspace(&self, project: &WorkspacePolicy) -> anyhow::Result<WorkspacePolicy> {
        let id = uuid::Uuid::parse_str(&self.id)?.to_string();
        let expected = project.root().join(".joe-worktrees").join(&id);
        match self.path == expected && self.target == "main" && project.is_directory(&expected)? {
            true => Ok(()),
            false => Err(anyhow::anyhow!(
                "Session worktree identity changed; retained for inspection"
            )),
        }?;
        let source = GitRepository::source(project)?;
        let workspace = WorkspacePolicy::workspace(expected)?;
        let git = GitRepository::required(&workspace)?;
        let reference = format!("refs/heads/{}", self.branch());
        match git.repo.path().canonicalize()?
            == source
                .repo
                .commondir()
                .join("worktrees")
                .join(id)
                .canonicalize()?
            && git.repo.head()?.name()? == reference
        {
            true => Ok(workspace),
            false => Err(anyhow::anyhow!(
                "Session worktree branch or Git metadata changed"
            )),
        }
    }

    fn checkpoint(&self, project: &WorkspacePolicy) -> anyhow::Result<Oid> {
        self.commit_snapshot(project, None)
    }

    fn commit_snapshot(
        &self,
        project: &WorkspacePolicy,
        resolution: Option<&MergeConflict>,
    ) -> anyhow::Result<Oid> {
        let workspace = self.workspace(project)?;
        let mut git = GitRepository::required(&workspace)?;
        let mut index = git.repo.index()?;
        let state = git.repo.state();
        let permitted = match (resolution, state) {
            (None, RepositoryState::Clean) => !index.has_conflicts(),
            (Some(_), RepositoryState::Clean | RepositoryState::Merge) => true,
            _ => false,
        };
        match permitted {
            true => Ok(()),
            false => Err(anyhow::anyhow!(
                "Resolve the session's pending Git operation before merging"
            )),
        }?;
        let mut merge_heads = Vec::new();
        if state == RepositoryState::Merge {
            git.repo.mergehead_foreach(|head| {
                merge_heads.push(*head);
                true
            })?;
        }
        let reference = format!("refs/heads/{}", self.branch());
        let mut transaction = git.repo.transaction()?;
        transaction.lock_ref(&reference)?;
        let parent = git.repo.find_commit(git.repo.refname_to_id(&reference)?)?;
        let snapshot = WorktreeSnapshot::current(&workspace, &git)?;
        let target = resolution
            .map(|conflict| {
                ResolutionState::new(conflict, &parent, state, &merge_heads)?.target(&git.repo)
            })
            .transpose()?
            .flatten();
        let snapshot = match resolution {
            Some(conflict) => conflict.resolved_snapshot(snapshot)?,
            None => snapshot,
        };
        index.clear()?;
        for (path, version) in snapshot.files {
            let entry = git2::IndexEntry {
                ctime: git2::IndexTime::new(0, 0),
                mtime: git2::IndexTime::new(0, 0),
                dev: 0,
                ino: 0,
                mode: 0o100000 | version.mode(),
                uid: 0,
                gid: 0,
                file_size: version.bytes().len() as u32,
                id: Oid::ZERO_SHA1,
                flags: 0,
                flags_extended: 0,
                path: path.as_os_str().as_encoded_bytes().to_vec(),
            };
            index.add_frombuffer(&entry, version.bytes())?;
        }
        let tree = git.repo.find_tree(index.write_tree()?)?;
        let signature = Signature::now("Agent Joe", "agent-joe@localhost")?;
        let parents = std::iter::once(&parent)
            .chain(target.as_ref())
            .collect::<Vec<_>>();
        let commit = match tree.id() == parent.tree_id() && target.is_none() {
            true => parent.id(),
            false => {
                let before = target.as_ref().unwrap_or(&parent).tree()?;
                let summary = CommitSummary::new(&git.repo, &before, &tree)?;
                git.repo.commit(
                    None,
                    &signature,
                    &signature,
                    &summary.subject(),
                    &tree,
                    &parents,
                )?
            }
        };
        transaction.set_target(
            &reference,
            commit,
            Some(&signature),
            "Joe session checkpoint",
        )?;
        transaction.commit()?;
        index.write()?;
        if resolution.is_some() && state == RepositoryState::Merge {
            git.repo.cleanup_state()?;
        }
        Ok(commit)
    }

    pub fn prepare_resolution(
        &self,
        project: &WorkspacePolicy,
        conflict: &MergeConflict,
    ) -> anyhow::Result<()> {
        let workspace = self.workspace(project)?;
        let git = GitRepository::required(&workspace)?;
        let approved = git.repo.find_commit(Oid::from_str(&conflict.approved)?)?;
        let target = git.repo.find_commit(Oid::from_str(&conflict.target)?)?;
        match git.repo.head()?.peel_to_commit()?.id() == approved.id()
            && git.repo.state() == RepositoryState::Clean
            && git.status(&workspace)?.entries.is_empty()
        {
            true => Ok(()),
            false => Err(anyhow::anyhow!(
                "Session changed before conflict resolution could start"
            )),
        }?;
        let before = WorktreeSnapshot::base(&git, &conflict.approved)?;
        let incoming = WorktreeSnapshot::base(&git, &conflict.target)?;
        for path in super::snapshot::changed_paths(&before.files, &incoming.files) {
            workspace.check(&path, crate::workspace::Access::Write)?;
            match workspace.file_version(&path)?
                == *before
                    .files
                    .get(&path)
                    .unwrap_or(&crate::changes::FileVersion::Missing)
            {
                true => Ok(()),
                false => Err(anyhow::anyhow!(
                    "Session has local data at {}; conflict resolution cannot overwrite it",
                    path.display()
                )),
            }?;
        }
        let target = git.repo.find_annotated_commit(target.id())?;
        let mut checkout = git2::build::CheckoutBuilder::new();
        checkout
            .safe()
            .allow_conflicts(true)
            .conflict_style_diff3(true)
            .our_label("Joe session")
            .their_label("main")
            .overwrite_ignored(false);
        git.repo.merge(&[&target], None, Some(&mut checkout))?;
        Ok(())
    }

    pub fn finish_resolution(
        &self,
        project: &WorkspacePolicy,
        conflict: &MergeConflict,
    ) -> anyhow::Result<String> {
        self.commit_snapshot(project, Some(conflict))
            .map(|commit| commit.to_string())
    }

    pub fn proposal(&self, project: &WorkspacePolicy) -> anyhow::Result<Option<String>> {
        let commit = self.checkpoint(project)?;
        let git = GitRepository::source(project)?;
        let target = git.repo.refname_to_id("refs/heads/main")?;
        Ok(
            (target != commit && !git.repo.graph_descendant_of(target, commit)?)
                .then(|| commit.to_string()),
        )
    }

    pub fn merge(&self, project: &WorkspacePolicy, approved: &str) -> anyhow::Result<MergeOutcome> {
        self.merge_into_target(project, approved).with_context(|| {
            format!(
                "Session {} remains at {}. Could not merge into {}",
                self.id,
                self.path.display(),
                self.target
            )
        })
    }

    pub fn cleanup(&self, project: &WorkspacePolicy, approved: &str) -> anyhow::Result<()> {
        let git = GitRepository::source(project)?;
        SessionCleanup::new(self, project, &git, approved)?.execute()
    }

    fn merge_into_target(
        &self,
        project: &WorkspacePolicy,
        approved: &str,
    ) -> anyhow::Result<MergeOutcome> {
        let commit = self.checkpoint(project)?;
        match commit.to_string() == approved {
            true => Ok(()),
            false => Err(anyhow::anyhow!(
                "Session changes have changed since the merge question; complete the task again before approving"
            )),
        }?;
        let git = GitRepository::source(project)?;
        let reference = format!("refs/heads/{}", self.target);
        let mut transaction = git.repo.transaction()?;
        transaction.lock_ref(&reference)?;
        transaction.lock_ref("HEAD")?;
        let target = git.repo.find_commit(git.repo.refname_to_id(&reference)?)?;
        let state = match target.id() == commit
            || git.repo.graph_descendant_of(target.id(), commit)?
        {
            true => MergeState::Integrated,
            false if git.repo.graph_descendant_of(commit, target.id())? => MergeState::FastForward,
            false => MergeState::Diverged,
        };
        match state {
            MergeState::Integrated => Ok(MergeOutcome::Unchanged),
            MergeState::FastForward | MergeState::Diverged => {
                let checked_out = git.repo.head()?.name()? == reference;
                for name in git.repo.worktrees()?.iter() {
                    let name =
                        name?.ok_or_else(|| anyhow::anyhow!("Worktree name is not UTF-8"))?;
                    let linked = git2::Repository::open(git.repo.find_worktree(name)?.path())?;
                    match linked.head()?.name()? == reference {
                        true => Err(anyhow::anyhow!(
                            "{} is checked out in another worktree",
                            self.target
                        )),
                        false => Ok(()),
                    }?;
                }
                match checked_out
                    && (git.repo.state() != RepositoryState::Clean
                        || !git.status(project)?.entries.is_empty())
                {
                    true => Err(anyhow::anyhow!(
                        "{} has local changes or a pending Git operation",
                        self.target
                    )),
                    false => Ok(()),
                }?;
                let signature = Signature::now("Agent Joe", "agent-joe@localhost")?;
                let session = git.repo.find_commit(commit)?;
                let merged = match state {
                    MergeState::FastForward => commit,
                    MergeState::Diverged => {
                        let mut index = git.repo.merge_commits(&target, &session, None)?;
                        match index.has_conflicts() {
                            true => Err(anyhow::Error::new(MergeConflict::new(
                                commit,
                                target.id(),
                                &index,
                            )?)),
                            false => Ok(()),
                        }?;
                        let tree = git.repo.find_tree(index.write_tree_to(&git.repo)?)?;
                        let summary = CommitSummary::new(&git.repo, &target.tree()?, &tree)?;
                        git.repo.commit(
                            None,
                            &signature,
                            &signature,
                            &summary.subject(),
                            &tree,
                            &[&target, &session],
                        )?
                    }
                    MergeState::Integrated => target.id(),
                };
                let merged_commit = git.repo.find_commit(merged)?;
                if checked_out {
                    git.repo.checkout_tree(
                        merged_commit.as_object(),
                        Some(
                            git2::build::CheckoutBuilder::new()
                                .safe()
                                .overwrite_ignored(false),
                        ),
                    )?;
                }
                transaction.set_target(
                    &reference,
                    merged,
                    Some(&signature),
                    "Merge Joe session",
                )?;
                transaction.commit()?;
                Ok(MergeOutcome::Merged {
                    target: self.target.clone(),
                    commit: merged.to_string(),
                })
            }
        }
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/git/session_worktrees/tests.rs"]
mod tests;
