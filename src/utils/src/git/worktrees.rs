use super::{GitRepository, Revision};
use crate::{
    changes::{Baseline, ChangeTracker, FileEdit, FileVersion},
    workspace::{Access, WorkspacePolicy},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManagedWorktree {
    pub id: String,
    pub path: PathBuf,
    pub base: String,
    pub branch: String,
    pub dirty_source: bool,
    pub state: WorktreeState,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum WorktreeState {
    Creating,
    Active,
    Integrated {
        files: BTreeMap<PathBuf, FileVersion>,
        head: String,
    },
    Removing,
    Removed,
    Failed {
        message: String,
    },
}

#[derive(Clone, Copy)]
pub enum DirtySource {
    Reject,
    BaseOnly,
}

pub enum WorktreeOperation {
    Create { base: Revision, dirty: DirtySource },
    List,
    Integrate { id: String },
    Remove { id: String },
}

impl ManagedWorktree {
    pub fn execute(
        workspace: &WorkspacePolicy,
        tracker: &ChangeTracker,
        operation: WorktreeOperation,
    ) -> anyhow::Result<Vec<Self>> {
        let access = match operation {
            WorktreeOperation::List => Access::Read,
            _ => Access::Write,
        };
        let workspace = workspace
            .permits_workspace_access(access)
            .then_some(workspace)
            .ok_or_else(|| {
                anyhow::anyhow!("Worktree operations require whole-project path access")
            })?;
        match operation {
            WorktreeOperation::List => Ok(tracker.snapshot()?.worktrees),
            WorktreeOperation::Create { base, dirty } => {
                Self::create(workspace, tracker, base, dirty).map(|worktree| vec![worktree])
            }
            WorktreeOperation::Integrate { id } => Self::selected(workspace, tracker, &id)?
                .integrate(workspace, tracker)
                .map(|worktree| vec![worktree]),
            WorktreeOperation::Remove { id } => Self::selected(workspace, tracker, &id)?
                .remove(workspace, tracker)
                .map(|worktree| vec![worktree]),
        }
    }

    fn selected(
        workspace: &WorkspacePolicy,
        tracker: &ChangeTracker,
        id: &str,
    ) -> anyhow::Result<Self> {
        let record = tracker
            .snapshot()?
            .worktrees
            .into_iter()
            .find(|record| record.id == id)
            .ok_or_else(|| anyhow::anyhow!("Unknown managed worktree ID"))?;
        let uuid = uuid::Uuid::parse_str(id)?;
        match record.path
            == workspace
                .root()
                .join(".joe-worktrees")
                .join(uuid.to_string())
            && record.branch == format!("joe/{uuid}")
        {
            true => Ok(record),
            false => Err(anyhow::anyhow!(
                "Managed worktree path or branch does not match its saved identity"
            )),
        }
    }

    fn save(&self, tracker: &ChangeTracker) -> anyhow::Result<()> {
        let mut records = tracker.snapshot()?.worktrees;
        match records.iter().position(|record| record.id == self.id) {
            Some(index) => records[index] = self.clone(),
            None => records.push(self.clone()),
        }
        tracker.update_worktrees(records)
    }

    fn source(workspace: &WorkspacePolicy) -> anyhow::Result<GitRepository> {
        let git = GitRepository::required(workspace)?;
        match git
            .repo
            .commondir()
            .canonicalize()?
            .starts_with(workspace.root())
        {
            true => Ok(git),
            false => Err(anyhow::anyhow!(
                "Managed worktree mutations require the original repository root; this linked worktree has read-only shared Git metadata"
            )),
        }
    }

    fn create(
        workspace: &WorkspacePolicy,
        tracker: &ChangeTracker,
        base: Revision,
        dirty: DirtySource,
    ) -> anyhow::Result<Self> {
        let git = Self::source(workspace)?;
        let status = git.status(workspace)?;
        let dirty_source = !status.entries.is_empty();
        match (dirty_source, dirty) {
            (true, DirtySource::Reject) => Err(anyhow::anyhow!(
                "Source index or worktree is dirty. Choose base_only explicitly to isolate the selected commit without copying existing edits"
            )),
            _ => Ok(()),
        }?;
        let commit = git.commit(&base)?;
        let files = commit_files(&git.repo, commit.id())?;
        let id = uuid::Uuid::new_v4().to_string();
        let path = workspace.root().join(".joe-worktrees").join(&id);
        for name in files.keys() {
            workspace.check(&path.join(name), Access::Write)?;
        }
        workspace.create_parent_dirs(&path)?;
        let mut record = Self {
            id: id.clone(),
            path,
            base: commit.id().to_string(),
            branch: format!("joe/{id}"),
            dirty_source,
            state: WorktreeState::Creating,
        };
        record.save(tracker)?;
        let created = (|| {
            let branch = git.repo.branch(&record.branch, &commit, false)?;
            let mut options = git2::WorktreeAddOptions::new();
            options.reference(Some(branch.get()));
            git.repo.worktree(&id, &record.path, Some(&options))?;
            let child = WorkspacePolicy::workspace(record.path.clone())?;
            let snapshot = Baseline::capture(&child)?;
            match snapshot.files == files {
                true => Ok(()),
                false => Err(anyhow::anyhow!(
                    "Worktree checkout differs from the selected base; retained for inspection"
                )),
            }
        })();
        record.state = match &created {
            Ok(()) => WorktreeState::Active,
            Err(error) => WorktreeState::Failed {
                message: format!("{error:#}"),
            },
        };
        record.save(tracker)?;
        created?;
        Ok(record)
    }

    fn child_workspace(
        &self,
        workspace: &WorkspacePolicy,
        source: &GitRepository,
    ) -> anyhow::Result<WorkspacePolicy> {
        match workspace.is_directory(&self.path)? {
            true => Ok(()),
            false => Err(anyhow::anyhow!(
                "Managed worktree is not an ordinary project directory"
            )),
        }?;
        let child = WorkspacePolicy::workspace(self.path.clone())?;
        let git = GitRepository::required(&child)?;
        match git.repo.path().canonicalize()?
            == source
                .repo
                .commondir()
                .join("worktrees")
                .join(&self.id)
                .canonicalize()?
        {
            true => Ok(child),
            false => Err(anyhow::anyhow!("Managed worktree control metadata changed")),
        }
    }

    pub fn integration_paths(&self, workspace: &WorkspacePolicy) -> anyhow::Result<Vec<PathBuf>> {
        let git = Self::source(workspace)?;
        let base = commit_files(&git.repo, git2::Oid::from_str(&self.base)?)?;
        let child = self.child_workspace(workspace, &git)?;
        let current = Baseline::capture(&child)?.files;
        Ok(changed_paths(&base, &current))
    }

    fn integrate(
        mut self,
        workspace: &WorkspacePolicy,
        tracker: &ChangeTracker,
    ) -> anyhow::Result<Self> {
        match self.state {
            WorktreeState::Active | WorktreeState::Integrated { .. } => Ok(()),
            _ => Err(anyhow::anyhow!("Worktree is not available for integration")),
        }?;
        let git = Self::source(workspace)?;
        match git.head()?.as_deref() == Some(&self.base) {
            true => Ok(()),
            false => Err(anyhow::anyhow!(
                "Integration conflict: source HEAD moved since the selected base"
            )),
        }?;
        let base = commit_files(&git.repo, git2::Oid::from_str(&self.base)?)?;
        let child = self.child_workspace(workspace, &git)?;
        let child_git = GitRepository::required(&child)?;
        match child_git
            .status(&child)?
            .entries
            .iter()
            .all(|entry| !entry.conflicted)
        {
            true => Ok(()),
            false => Err(anyhow::anyhow!(
                "Resolve worktree index conflicts before integration"
            )),
        }?;
        match child_git
            .diff(&child, super::DiffTarget::Staged, None)?
            .is_empty()
        {
            true => Ok(()),
            false => Err(anyhow::anyhow!(
                "Worktree has staged changes; integration preserves index state by requiring committed or unstaged worktree edits"
            )),
        }?;
        let mut current = Baseline::capture(&child)?.files;
        current.retain(|_, version| *version != FileVersion::Missing);
        let changed = changed_paths(&base, &current);
        let status = git.status(workspace)?;
        let previous = match &self.state {
            WorktreeState::Integrated { files, .. } => files,
            _ => &base,
        };
        let paths = changed
            .into_iter()
            .chain(changed_paths(&base, previous))
            .collect::<BTreeSet<_>>();
        let edits = paths
            .into_iter()
            .map(|path| {
                match status.entries.iter().any(|entry| {
                    (entry.path == path || entry.previous_path.as_ref() == Some(&path))
                        && (entry.index != super::GitChange::Unchanged || entry.conflicted)
                }) {
                    true => Err(anyhow::anyhow!(
                        "Integration conflict with source index: {}",
                        path.display()
                    )),
                    false => Ok(()),
                }?;
                let before = previous.get(&path).cloned().unwrap_or(FileVersion::Missing);
                let after = current.get(&path).cloned().unwrap_or(FileVersion::Missing);
                FileEdit::new(workspace, &path, before, after)
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        if !edits.is_empty() {
            tracker.apply(workspace, edits)?;
        }
        self.state = WorktreeState::Integrated {
            files: current,
            head: child_git
                .head()?
                .ok_or_else(|| anyhow::anyhow!("Worktree HEAD is missing"))?,
        };
        self.save(tracker)?;
        Ok(self)
    }

    fn remove(
        mut self,
        workspace: &WorkspacePolicy,
        tracker: &ChangeTracker,
    ) -> anyhow::Result<Self> {
        let git = Self::source(workspace)?;
        let child = self.child_workspace(workspace, &git)?;
        let child_git = GitRepository::required(&child)?;
        let expected = match &self.state {
            WorktreeState::Active => commit_files(&git.repo, git2::Oid::from_str(&self.base)?)?,
            WorktreeState::Integrated { files, .. } => files.clone(),
            _ => Err(anyhow::anyhow!(
                "Incomplete worktree operation requires inspection; automatic cleanup is unavailable"
            ))?,
        };
        let expected_head = match &self.state {
            WorktreeState::Integrated { head, .. } => head,
            _ => &self.base,
        };
        let actual = all_files(&child)?;
        match actual == expected
            && child_git.head()?.as_ref() == Some(expected_head)
            && !child_git.repo.index()?.has_conflicts()
            && child_git
                .diff(&child, super::DiffTarget::Staged, None)?
                .is_empty()
        {
            true => Ok(()),
            false => Err(anyhow::anyhow!(
                "Cleanup conflict: worktree has edits, commits, ignored files, or private session data that have not been integrated"
            )),
        }?;
        let mut branch = git
            .repo
            .find_branch(&self.branch, git2::BranchType::Local)?;
        match branch.get().target().map(|id| id.to_string()).as_ref() == Some(expected_head) {
            true => Ok(()),
            false => Err(anyhow::anyhow!("Cleanup conflict: managed branch changed")),
        }?;
        self.state = WorktreeState::Removing;
        self.save(tracker)?;
        let mut options = git2::WorktreePruneOptions::new();
        options.valid(true).working_tree(true);
        git.repo
            .find_worktree(&self.id)?
            .prune(Some(&mut options))?;
        branch.delete()?;
        self.state = WorktreeState::Removed;
        self.save(tracker)?;
        Ok(self)
    }
}

fn changed_paths(
    before: &BTreeMap<PathBuf, FileVersion>,
    after: &BTreeMap<PathBuf, FileVersion>,
) -> Vec<PathBuf> {
    before
        .keys()
        .chain(after.keys())
        .filter(|path| before.get(*path) != after.get(*path))
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn commit_files(
    repo: &git2::Repository,
    id: git2::Oid,
) -> anyhow::Result<BTreeMap<PathBuf, FileVersion>> {
    let tree = repo.find_commit(id)?.tree()?;
    let mut files = BTreeMap::new();
    let mut bytes = 0usize;
    collect_tree(repo, &tree, Path::new(""), &mut files, &mut bytes)?;
    Ok(files)
}

fn collect_tree(
    repo: &git2::Repository,
    tree: &git2::Tree<'_>,
    root: &Path,
    files: &mut BTreeMap<PathBuf, FileVersion>,
    bytes: &mut usize,
) -> anyhow::Result<()> {
    tree.iter().try_for_each(|entry| {
        let path = root.join(entry.name()?);
        match entry.filemode() {
            0o040000 => collect_tree(repo, &repo.find_tree(entry.id())?, &path, files, bytes),
            0o100644 | 0o100755 => {
                let blob = repo.find_blob(entry.id())?;
                *bytes += blob.size();
                match !super::excluded(&path) && *bytes <= 64 * 1024 * 1024 && blob.size() <= 16 * 1024 * 1024 && files.len() < 250_000 {
                    true => Ok(()),
                    false => Err(anyhow::anyhow!("Selected tree contains a protected path or exceeds snapshot limits")),
                }?;
                files.insert(path, FileVersion::File { content: blob.content().to_vec(), mode: (entry.filemode() & 0o777) as u32 });
                Ok(())
            }
            _ => Err(anyhow::anyhow!("Managed checkout requires ordinary files; symlinks and submodules are unsupported: {}", path.display())),
        }
    })
}

fn all_files(workspace: &WorkspacePolicy) -> anyhow::Result<BTreeMap<PathBuf, FileVersion>> {
    let mut pending = vec![workspace.root().to_path_buf()];
    let mut files = BTreeMap::new();
    let mut bytes = 0usize;
    let mut visited = 0usize;
    while let Some(directory) = pending.pop() {
        for entry in workspace.entries(&directory)? {
            visited += 1;
            match visited <= 250_000 {
                true => Ok(()),
                false => Err(anyhow::anyhow!("Worktree cleanup exceeds the entry limit")),
            }?;
            match (
                entry.path == workspace.root().join(".git"),
                workspace.is_directory(&entry.path)?,
            ) {
                (true, _) => {}
                (_, true) => pending.push(entry.path),
                (_, false) => {
                    let version = workspace.file_version(&entry.path)?;
                    bytes += version.bytes().len();
                    match bytes <= 64 * 1024 * 1024 {
                        true => Ok(()),
                        false => Err(anyhow::anyhow!(
                            "Worktree cleanup exceeds the content limit"
                        )),
                    }?;
                    files.insert(workspace.relative_path(&entry.path, Access::Read)?, version);
                }
            }
        }
    }
    Ok(files)
}
