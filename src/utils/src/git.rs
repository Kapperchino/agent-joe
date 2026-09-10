use crate::workspace::{Access, WorkspacePolicy};
use control::{ControlDirectory, RepositoryLayout, initialize};
use git2::{Repository, RepositoryOpenFlags, StatusOptions};
use output::{BlobContent, diff_options, render_diff};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    path::{Component, Path, PathBuf},
};

mod control;
mod output;
pub mod worktrees;

const OUTPUT_LIMIT: usize = 32 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(try_from = "String")]
pub struct Revision(String);

impl Revision {
    pub fn new(value: &str) -> anyhow::Result<Self> {
        let valid = !value.is_empty()
            && value.len() <= 200
            && !value.starts_with(['-', '/'])
            && !value.contains("..")
            && !value.contains("@{")
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"/._-~^".contains(&byte));
        match valid {
            true => Ok(Self(value.to_owned())),
            false => Err(anyhow::anyhow!(
                "Use a commit ID or a simple reference, optionally followed by ~ or ^ ancestry"
            )),
        }
    }
}

impl TryFrom<String> for Revision {
    type Error = anyhow::Error;

    fn try_from(value: String) -> anyhow::Result<Self> {
        Self::new(&value)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct LogLimit(usize);

impl LogLimit {
    pub fn new(value: usize) -> anyhow::Result<Self> {
        match value {
            1..=100 => Ok(Self(value)),
            _ => Err(anyhow::anyhow!("Log limit must be between 1 and 100")),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiffTarget {
    Staged,
    Unstaged,
    Head,
}

impl DiffTarget {
    pub fn new(value: &str) -> anyhow::Result<Self> {
        match value {
            "staged" => Ok(Self::Staged),
            "unstaged" => Ok(Self::Unstaged),
            "head" => Ok(Self::Head),
            _ => Err(anyhow::anyhow!(
                "Diff target must be staged, unstaged, or head"
            )),
        }
    }
}

pub enum GitOperation {
    Status,
    Diff {
        target: DiffTarget,
        path: Option<PathBuf>,
    },
    Show {
        revision: Revision,
        path: Option<PathBuf>,
    },
    Log {
        revision: Revision,
        limit: LogLimit,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitStatus {
    pub head: Option<String>,
    pub entries: Vec<StatusEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusEntry {
    pub path: PathBuf,
    pub previous_path: Option<PathBuf>,
    pub index: GitChange,
    pub worktree: GitChange,
    pub conflicted: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GitChange {
    Unchanged,
    Added,
    Deleted,
    Renamed,
    TypeChanged,
    Modified,
    Untracked,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitInfo {
    pub id: String,
    pub parents: Vec<String>,
    pub author: String,
    pub time: i64,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "operation", content = "result", rename_all = "snake_case")]
pub enum GitResult {
    Status(GitStatus),
    Diff(String),
    Show { commit: CommitInfo, content: String },
    Log(Vec<CommitInfo>),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexEntry {
    pub path: PathBuf,
    pub blob: String,
    pub mode: u32,
    pub flags: u16,
    pub extended_flags: u16,
}

#[derive(PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct IndexKey {
    path: PathBuf,
    stage: u16,
}

impl IndexEntry {
    pub(crate) fn key(&self) -> IndexKey {
        IndexKey {
            path: self.path.clone(),
            stage: (self.flags >> 12) & 3,
        }
    }
}

pub struct GitRepository {
    pub(crate) repo: Repository,
}

impl GitRepository {
    pub fn open(workspace: &WorkspacePolicy) -> anyhow::Result<Option<Self>> {
        RepositoryLayout::discover(workspace)?
            .map(|layout| Self::from_layout(workspace, layout))
            .transpose()
    }

    fn from_layout(workspace: &WorkspacePolicy, layout: RepositoryLayout) -> anyhow::Result<Self> {
        initialize()?;
        let common = ControlDirectory::new(layout.common)?;
        let repo = Repository::open_ext(
            &layout.control,
            RepositoryOpenFlags::NO_SEARCH | RepositoryOpenFlags::NO_DOTGIT,
            std::iter::empty::<&Path>(),
        )?;
        repo.set_config(&git2::Config::new()?)?;
        repo.add_ignore_rule(".[tT][uU][rR][bB][oO]-[cC][oO][dD][eE]/\n.[jJ][oO][eE]-[wW][oO][rR][kK][tT][rR][eE][eE][sS]/")?;
        let workdir = repo
            .workdir()
            .ok_or_else(|| anyhow::anyhow!("Bare repositories are not task workspaces"))?
            .canonicalize()?;
        match workdir {
            workdir if workdir != workspace.root() => Err(anyhow::anyhow!(
                "Git workdir differs from the fixed workspace"
            )),
            _ if repo.commondir().canonicalize()? != common.path => Err(anyhow::anyhow!(
                "Git common directory differs from the validated metadata"
            )),
            _ => Ok(Self { repo }),
        }
    }

    pub fn required(workspace: &WorkspacePolicy) -> anyhow::Result<Self> {
        Self::open(workspace)?
            .ok_or_else(|| anyhow::anyhow!("The workspace root is not a Git repository"))
    }

    fn source(workspace: &WorkspacePolicy) -> anyhow::Result<Self> {
        let git = Self::required(workspace)?;
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

    pub fn execute(
        workspace: &WorkspacePolicy,
        operation: GitOperation,
    ) -> anyhow::Result<GitResult> {
        let git = Self::required(workspace)?;
        match operation {
            GitOperation::Status => git.status(workspace).map(GitResult::Status),
            GitOperation::Diff { target, path } => git
                .diff(workspace, target, path.as_deref())
                .map(GitResult::Diff),
            GitOperation::Show { revision, path } => {
                git.show(workspace, &revision, path.as_deref())
            }
            GitOperation::Log { revision, limit } => git.log(&revision, limit).map(GitResult::Log),
        }?
        .bounded()
    }

    fn show(
        &self,
        workspace: &WorkspacePolicy,
        revision: &Revision,
        path: Option<&Path>,
    ) -> anyhow::Result<GitResult> {
        let commit = self.commit(revision)?;
        let tree = commit.tree()?;
        let content = match path {
            Some(path) => {
                let path = GitPath::new(workspace, path)?;
                let entry = tree.get_path(&path.path)?;
                BlobContent::new(&self.repo, entry.id())?.text
            }
            None => {
                let parent = commit
                    .parents()
                    .next()
                    .map(|parent| parent.tree())
                    .transpose()?;
                let diff = self.repo.diff_tree_to_tree(
                    parent.as_ref(),
                    Some(&tree),
                    Some(&mut diff_options()),
                )?;
                render_diff(&diff)?
            }
        };
        Ok(GitResult::Show {
            commit: CommitInfo::from(&commit),
            content,
        })
    }

    fn log(&self, revision: &Revision, limit: LogLimit) -> anyhow::Result<Vec<CommitInfo>> {
        let mut walk = self.repo.revwalk()?;
        walk.set_sorting(git2::Sort::TOPOLOGICAL | git2::Sort::TIME)?;
        walk.push(self.commit(revision)?.id())?;
        walk.take(limit.0)
            .map(|id| Ok(CommitInfo::from(&self.repo.find_commit(id?)?)))
            .collect()
    }

    pub(crate) fn commit(&self, revision: &Revision) -> anyhow::Result<git2::Commit<'_>> {
        Ok(self.repo.revparse_single(&revision.0)?.peel_to_commit()?)
    }

    pub fn head(&self) -> anyhow::Result<Option<String>> {
        match self.repo.head() {
            Ok(head) => Ok(Some(head.peel_to_commit()?.id().to_string())),
            Err(error)
                if matches!(
                    error.code(),
                    git2::ErrorCode::UnbornBranch | git2::ErrorCode::NotFound
                ) =>
            {
                Ok(None)
            }
            Err(error) => Err(error.into()),
        }
    }

    pub fn index(&self, workspace: &WorkspacePolicy) -> anyhow::Result<Vec<IndexEntry>> {
        self.repo
            .index()?
            .iter()
            .map(|entry| IndexEntry::new(workspace, &entry))
            .collect()
    }

    pub fn paths(&self, workspace: &WorkspacePolicy) -> anyhow::Result<Vec<PathBuf>> {
        let tracked = self
            .repo
            .index()?
            .iter()
            .map(|entry| {
                GitPath::new(workspace, Path::new(std::str::from_utf8(&entry.path)?))
                    .map(|path| path.path)
            })
            .collect::<anyhow::Result<BTreeSet<_>>>()?;
        Ok(tracked
            .into_iter()
            .chain(
                self.status(workspace)?
                    .entries
                    .into_iter()
                    .map(|entry| entry.path),
            )
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect())
    }

    pub fn status(&self, workspace: &WorkspacePolicy) -> anyhow::Result<GitStatus> {
        crate::inventory::Inventory::scan_git(workspace)?;
        self.repo.index()?.iter().try_for_each(|entry| {
            GitPath::tracked(workspace, Path::new(std::str::from_utf8(&entry.path)?)).map(|_| ())
        })?;
        let mut options = StatusOptions::new();
        options
            .include_untracked(true)
            .recurse_untracked_dirs(true)
            .renames_head_to_index(true)
            .renames_index_to_workdir(true)
            .update_index(false)
            .exclude_submodules(true);
        let entries = self
            .repo
            .statuses(Some(&mut options))?
            .iter()
            .filter(|entry| !entry.path().is_ok_and(|path| excluded(Path::new(path))))
            .map(|entry| StatusEntry::new(workspace, &entry))
            .collect::<anyhow::Result<Vec<_>>>()?;
        Ok(GitStatus {
            head: self.head()?,
            entries,
        })
    }

    pub fn diff(
        &self,
        workspace: &WorkspacePolicy,
        target: DiffTarget,
        path: Option<&Path>,
    ) -> anyhow::Result<String> {
        let paths = match path {
            Some(path) => vec![GitPath::new(workspace, path)?.path],
            None => self.paths(workspace)?,
        };
        let mut options = paths.iter().try_fold(diff_options(), |mut options, path| {
            workspace.file_version(path)?;
            options.pathspec(path);
            Ok::<_, anyhow::Error>(options)
        })?;
        let tree = self
            .head()?
            .map(|id| self.repo.find_commit(git2::Oid::from_str(&id)?)?.tree())
            .transpose()?;
        match paths.is_empty() {
            true => Ok(String::new()),
            false => {
                let mut diff = match target {
                    DiffTarget::Staged => {
                        self.repo
                            .diff_tree_to_index(tree.as_ref(), None, Some(&mut options))?
                    }
                    DiffTarget::Unstaged => {
                        self.repo.diff_index_to_workdir(None, Some(&mut options))?
                    }
                    DiffTarget::Head => self
                        .repo
                        .diff_tree_to_workdir_with_index(tree.as_ref(), Some(&mut options))?,
                };
                diff.find_similar(Some(git2::DiffFindOptions::new().renames(true)))?;
                render_diff(&diff)
            }
        }
    }
}

impl IndexEntry {
    fn new(workspace: &WorkspacePolicy, entry: &git2::IndexEntry) -> anyhow::Result<Self> {
        Ok(Self {
            path: GitPath::new(workspace, Path::new(std::str::from_utf8(&entry.path)?))?.path,
            blob: entry.id.to_string(),
            mode: entry.mode,
            flags: entry.flags,
            extended_flags: entry.flags_extended,
        })
    }
}

impl StatusEntry {
    fn new(workspace: &WorkspacePolicy, entry: &git2::StatusEntry<'_>) -> anyhow::Result<Self> {
        let status = entry.status();
        let delta = entry.index_to_workdir().or_else(|| entry.head_to_index());
        let path = delta
            .as_ref()
            .and_then(|delta| delta.new_file().path().or_else(|| delta.old_file().path()))
            .or_else(|| entry.path().ok().map(Path::new))
            .ok_or_else(|| anyhow::anyhow!("Git returned a non-UTF-8 path"))?;
        let path = GitPath::new(workspace, path)?.path;
        let previous_path = delta
            .filter(|delta| delta.status() == git2::Delta::Renamed)
            .and_then(|delta| delta.old_file().path().map(Path::to_path_buf))
            .map(|path| GitPath::new(workspace, &path).map(|path| path.path))
            .transpose()?;
        Ok(Self {
            path,
            previous_path,
            index: GitChange::from_status(status, StatusArea::Index),
            worktree: GitChange::from_status(status, StatusArea::Worktree),
            conflicted: status.is_conflicted(),
        })
    }
}

impl GitStatus {
    fn index_conflicts_with(&self, path: &Path) -> bool {
        self.entries.iter().any(|entry| {
            (entry.path == path || entry.previous_path.as_deref() == Some(path))
                && (entry.index != GitChange::Unchanged || entry.conflicted)
        })
    }
}

impl From<&git2::Commit<'_>> for CommitInfo {
    fn from(commit: &git2::Commit<'_>) -> Self {
        Self {
            id: commit.id().to_string(),
            parents: commit.parent_ids().map(|id| id.to_string()).collect(),
            author: commit.author().name().unwrap_or_default().to_owned(),
            time: commit.time().seconds(),
            message: String::from_utf8_lossy(commit.message_bytes()).into_owned(),
        }
    }
}

pub(crate) struct GitPath {
    pub path: PathBuf,
}

impl GitPath {
    pub(crate) fn new(workspace: &WorkspacePolicy, path: &Path) -> anyhow::Result<Self> {
        let relative = workspace.relative_path(path, Access::Read)?;
        let valid = !relative.as_os_str().is_empty()
            && relative
                .components()
                .all(|component| matches!(component, Component::Normal(_)))
            && !excluded(&relative);
        match valid {
            true => Ok(Self { path: relative }),
            false => Err(anyhow::anyhow!(
                "Git path is protected or empty: {}",
                path.display()
            )),
        }
    }

    fn tracked(workspace: &WorkspacePolicy, path: &Path) -> anyhow::Result<Self> {
        let path = Self::new(workspace, path)?;
        match workspace.file_size(&path.path) {
            Ok(size) if size <= 16 * 1024 * 1024 => Ok(path),
            Ok(_) => Err(anyhow::anyhow!(
                "Tracked Git input exceeds the 16 MiB file limit"
            )),
            Err(error)
                if error
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound) =>
            {
                Ok(path)
            }
            Err(error) => Err(error),
        }
    }
}

pub(crate) fn excluded(path: &Path) -> bool {
    path.components().any(|component| match component {
        Component::Normal(name) => [".git", ".turbo-code", ".joe-worktrees"]
            .iter()
            .any(|excluded| name.eq_ignore_ascii_case(excluded)),
        _ => false,
    })
}

enum StatusArea {
    Index,
    Worktree,
}

impl GitChange {
    fn from_status(status: git2::Status, area: StatusArea) -> Self {
        let changes = match area {
            StatusArea::Index => [
                (git2::Status::INDEX_NEW, GitChange::Added),
                (git2::Status::INDEX_DELETED, GitChange::Deleted),
                (git2::Status::INDEX_RENAMED, GitChange::Renamed),
                (git2::Status::INDEX_TYPECHANGE, GitChange::TypeChanged),
                (git2::Status::INDEX_MODIFIED, GitChange::Modified),
            ],
            StatusArea::Worktree => [
                (git2::Status::WT_NEW, GitChange::Untracked),
                (git2::Status::WT_DELETED, GitChange::Deleted),
                (git2::Status::WT_RENAMED, GitChange::Renamed),
                (git2::Status::WT_TYPECHANGE, GitChange::TypeChanged),
                (git2::Status::WT_MODIFIED, GitChange::Modified),
            ],
        };
        changes
            .into_iter()
            .find(|(flag, _)| status.contains(*flag))
            .map(|(_, label)| label)
            .unwrap_or(GitChange::Unchanged)
    }
}

#[cfg(test)]
pub(crate) mod tests;
