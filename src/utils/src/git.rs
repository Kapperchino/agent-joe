use crate::workspace::{Access, WorkspacePolicy};
use git2::{DiffFormat, DiffOptions, Repository, RepositoryOpenFlags, StatusOptions};
use serde::{Deserialize, Serialize};
use std::path::{Component, Path, PathBuf};

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
        let workspace = workspace
            .permits_workspace_access(Access::Read)
            .then_some(workspace)
            .ok_or_else(|| {
                anyhow::anyhow!("Git and aggregate review require whole-project path access")
            })?;
        initialize()?;
        let dotgit = workspace.root().join(".git");
        match std::fs::symlink_metadata(&dotgit) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
            Ok(metadata) => {
                let control = match metadata.is_dir() {
                    true => dotgit.clone(),
                    false if metadata.is_file() => {
                        let text = workspace.read(Path::new(".git"))?;
                        let path = text
                            .trim()
                            .strip_prefix("gitdir: ")
                            .ok_or_else(|| anyhow::anyhow!("Invalid linked-worktree metadata"))?;
                        let control = workspace.root().join(path).canonicalize()?;
                        let common = control.join("../..").canonicalize()?;
                        let backlink = ordinary_text(&control.join("gitdir"))?;
                        let common_link = ordinary_text(&control.join("commondir"))?;
                        match control.parent().and_then(Path::file_name)
                            == Some(std::ffi::OsStr::new("worktrees"))
                            && common.file_name() == Some(std::ffi::OsStr::new(".git"))
                            && Path::new(backlink.trim()).canonicalize()?
                                == dotgit.canonicalize()?
                            && control.join(common_link.trim()).canonicalize()? == common
                        {
                            true => control,
                            false => Err(anyhow::anyhow!(
                                "Linked worktree does not have matching Git control metadata"
                            ))?,
                        }
                    }
                    false => Err(anyhow::anyhow!(
                        "Git metadata must be an ordinary file or directory"
                    ))?,
                };
                let common = match control.parent().and_then(Path::file_name) {
                    Some(name) if name == "worktrees" => control.join("../..").canonicalize()?,
                    _ => control.clone(),
                };
                ControlDirectory::new(&common)?;
                let repo = Repository::open_ext(
                    &control,
                    RepositoryOpenFlags::NO_SEARCH | RepositoryOpenFlags::NO_DOTGIT,
                    std::iter::empty::<&Path>(),
                )?;
                repo.set_config(&git2::Config::new()?)?;
                repo.add_ignore_rule(".[tT][uU][rR][bB][oO]-[cC][oO][dD][eE]/\n.[jJ][oO][eE]-[wW][oO][rR][kK][tT][rR][eE][eE][sS]/")?;
                let workdir = repo
                    .workdir()
                    .ok_or_else(|| anyhow::anyhow!("Bare repositories are not task workspaces"))?
                    .canonicalize()?;
                match workdir == workspace.root() {
                    true => Ok(Some(Self { repo })),
                    false => Err(anyhow::anyhow!(
                        "Git workdir differs from the fixed workspace"
                    )),
                }
            }
        }
    }

    pub fn required(workspace: &WorkspacePolicy) -> anyhow::Result<Self> {
        Self::open(workspace)?
            .ok_or_else(|| anyhow::anyhow!("The workspace root is not a Git repository"))
    }

    pub fn execute(
        workspace: &WorkspacePolicy,
        operation: GitOperation,
    ) -> anyhow::Result<GitResult> {
        let git = Self::required(workspace)?;
        let result = match operation {
            GitOperation::Status => git.status(workspace).map(GitResult::Status),
            GitOperation::Diff { target, path } => git
                .diff(workspace, target, path.as_deref())
                .map(GitResult::Diff),
            GitOperation::Show { revision, path } => {
                let commit = git.commit(&revision)?;
                let content = match path {
                    Some(path) => {
                        let path = GitPath::new(workspace, &path)?;
                        let entry = commit.tree()?.get_path(&path.0)?;
                        match git.repo.odb()?.read_header(entry.id())?.0 <= OUTPUT_LIMIT {
                            true => Ok(()),
                            false => {
                                Err(anyhow::anyhow!("Git blob exceeds the 32 MiB output limit"))
                            }
                        }?;
                        let blob = git.repo.find_blob(entry.id())?;
                        bounded_text(blob.content())?
                    }
                    None => {
                        let tree = commit.tree()?;
                        let parent = commit
                            .parents()
                            .next()
                            .map(|parent| parent.tree())
                            .transpose()?;
                        let diff = git.repo.diff_tree_to_tree(
                            parent.as_ref(),
                            Some(&tree),
                            Some(&mut diff_options()),
                        )?;
                        render_diff(&diff)?
                    }
                };
                Ok(GitResult::Show {
                    commit: commit_info(&commit),
                    content,
                })
            }
            GitOperation::Log { revision, limit } => {
                let mut walk = git.repo.revwalk()?;
                walk.set_sorting(git2::Sort::TOPOLOGICAL | git2::Sort::TIME)?;
                walk.push(git.commit(&revision)?.id())?;
                let commits = walk
                    .take(limit.0)
                    .map(|id| Ok(commit_info(&git.repo.find_commit(id?)?)))
                    .collect::<anyhow::Result<Vec<_>>>()?;
                match serde_json::to_vec(&commits)?.len() <= OUTPUT_LIMIT {
                    true => Ok(GitResult::Log(commits)),
                    false => Err(anyhow::anyhow!("Git log exceeds the 32 MiB output limit")),
                }
            }
        }?;
        match serde_json::to_vec(&result)?.len() <= OUTPUT_LIMIT {
            true => Ok(result),
            false => Err(anyhow::anyhow!(
                "Git result exceeds the 32 MiB output limit"
            )),
        }
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
            .map(|entry| {
                Ok(IndexEntry {
                    path: GitPath::new(workspace, Path::new(std::str::from_utf8(&entry.path)?))?.0,
                    blob: entry.id.to_string(),
                    mode: entry.mode,
                    flags: entry.flags,
                    extended_flags: entry.flags_extended,
                })
            })
            .collect()
    }

    pub fn paths(&self, workspace: &WorkspacePolicy) -> anyhow::Result<Vec<PathBuf>> {
        let index = self.repo.index()?;
        let mut paths = index
            .iter()
            .map(|entry| {
                let path = PathBuf::from(String::from_utf8(entry.path)?);
                GitPath::new(workspace, &path).map(|path| path.0)
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        paths.extend(
            self.status(workspace)?
                .entries
                .into_iter()
                .map(|entry| entry.path),
        );
        paths.sort();
        paths.dedup();
        Ok(paths)
    }

    pub fn status(&self, workspace: &WorkspacePolicy) -> anyhow::Result<GitStatus> {
        crate::inventory::Inventory::scan_git(workspace)?;
        for entry in self.repo.index()?.iter() {
            let path = GitPath::new(workspace, Path::new(std::str::from_utf8(&entry.path)?))?;
            match workspace.file_size(&path.0) {
                Ok(size) if size <= 16 * 1024 * 1024 => Ok(()),
                Ok(_) => Err(anyhow::anyhow!(
                    "Tracked Git input exceeds the 16 MiB file limit"
                )),
                Err(error)
                    if error
                        .downcast_ref::<std::io::Error>()
                        .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound) =>
                {
                    Ok(())
                }
                Err(error) => Err(error),
            }?;
        }
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
            .map(|entry| {
                let status = entry.status();
                let delta = entry.index_to_workdir().or_else(|| entry.head_to_index());
                let path = delta
                    .as_ref()
                    .and_then(|delta| delta.new_file().path().or_else(|| delta.old_file().path()))
                    .or_else(|| entry.path().ok().map(Path::new))
                    .ok_or_else(|| anyhow::anyhow!("Git returned a non-UTF-8 path"))?;
                let path = GitPath::new(workspace, path)?.0;
                let previous_path = delta
                    .filter(|delta| delta.status() == git2::Delta::Renamed)
                    .and_then(|delta| delta.old_file().path().map(Path::to_path_buf));
                Ok(StatusEntry {
                    path,
                    previous_path,
                    index: GitChange::from_status(status, StatusArea::Index),
                    worktree: GitChange::from_status(status, StatusArea::Worktree),
                    conflicted: status.is_conflicted(),
                })
            })
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
        let mut options = diff_options();
        let paths = match path {
            Some(path) => vec![GitPath::new(workspace, path)?.0],
            None => self.paths(workspace)?,
        };
        for path in &paths {
            workspace.file_version(path)?;
            options.pathspec(path);
        }
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

pub(crate) struct GitPath(pub PathBuf);

impl GitPath {
    pub(crate) fn new(workspace: &WorkspacePolicy, path: &Path) -> anyhow::Result<Self> {
        let relative = workspace.relative_path(path, Access::Read)?;
        match !relative.as_os_str().is_empty()
            && relative
                .components()
                .all(|component| matches!(component, Component::Normal(_)))
            && !excluded(&relative)
        {
            true => Ok(Self(relative)),
            false => Err(anyhow::anyhow!(
                "Git path is protected or empty: {}",
                path.display()
            )),
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

struct ControlDirectory;

impl ControlDirectory {
    fn new(root: &Path) -> anyhow::Result<Self> {
        let mut pending = vec![root.to_path_buf()];
        let mut count = 0usize;
        while let Some(path) = pending.pop() {
            let metadata = std::fs::symlink_metadata(&path)?;
            count += 1;
            match !metadata.is_symlink() && count <= 250_000 {
                true => Ok(()),
                false => Err(anyhow::anyhow!(
                    "Git metadata has a symlink or exceeds 250000 entries"
                )),
            }?;
            match (metadata.is_dir(), metadata.is_file()) {
                (true, _) => pending.extend(
                    std::fs::read_dir(&path)?
                        .map(|entry| entry.map(|entry| entry.path()))
                        .collect::<Result<Vec<_>, _>>()?,
                ),
                (_, true) => {
                    if path.file_name().is_some_and(|name| {
                        name.eq_ignore_ascii_case("config")
                            || name.eq_ignore_ascii_case("config.worktree")
                    }) {
                        let config = ordinary_text(&path)?;
                        let includes = config
                            .lines()
                            .map(str::trim)
                            .map(str::to_ascii_lowercase)
                            .any(|line| {
                                line.starts_with('[')
                                    && line
                                        .trim_start_matches('[')
                                        .trim_start()
                                        .starts_with("include")
                            });
                        match includes {
                            true => Err(anyhow::anyhow!(
                                "Git config includes are unavailable inside the fixed project boundary"
                            )),
                            false => Ok(()),
                        }?;
                    }
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::MetadataExt;
                        match metadata.nlink() == 1 {
                            true => Ok(()),
                            false => {
                                Err(anyhow::anyhow!("Git metadata hard links are unsupported"))
                            }
                        }?;
                    }
                    let normalized = path.to_string_lossy().to_ascii_lowercase();
                    if normalized.ends_with("/objects/info/alternates")
                        || normalized.ends_with("/objects/info/http-alternates")
                    {
                        match metadata.len() == 0 {
                            true => Ok(()),
                            false => Err(anyhow::anyhow!(
                                "External Git object stores are unavailable"
                            )),
                        }?;
                    }
                }
                _ => Err(anyhow::anyhow!("Git metadata contains a special file"))?,
            }
        }
        Ok(Self)
    }
}

fn ordinary_text(path: &Path) -> anyhow::Result<String> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Missing Git metadata parent"))?;
    let workspace = WorkspacePolicy::workspace(parent.to_path_buf())?;
    workspace.read(path)
}

fn diff_options() -> DiffOptions {
    let mut options = DiffOptions::new();
    options
        .include_untracked(true)
        .recurse_untracked_dirs(true)
        .show_untracked_content(true)
        .disable_pathspec_match(true)
        .ignore_submodules(true)
        .skip_binary_check(false);
    options
}

fn render_diff(diff: &git2::Diff<'_>) -> anyhow::Result<String> {
    let mut bytes = Vec::new();
    let mut exceeded = false;
    let result = diff.print(DiffFormat::Patch, |delta, _, line| {
        let hidden = delta
            .new_file()
            .path()
            .or_else(|| delta.old_file().path())
            .is_some_and(excluded);
        let prefix = matches!(line.origin(), '+' | '-' | ' ');
        let fits = bytes
            .len()
            .saturating_add(line.content().len() + usize::from(prefix))
            <= OUTPUT_LIMIT;
        match (hidden, fits) {
            (true, _) => true,
            (false, true) => {
                bytes.extend(
                    prefix
                        .then_some(line.origin() as u8)
                        .into_iter()
                        .chain(line.content().iter().copied()),
                );
                true
            }
            (false, false) => {
                exceeded = true;
                false
            }
        }
    });
    match exceeded {
        true => Err(anyhow::anyhow!("Git diff exceeds the 32 MiB output limit")),
        false => {
            result?;
            Ok(String::from_utf8_lossy(&bytes).into_owned())
        }
    }
}

fn bounded_text(bytes: &[u8]) -> anyhow::Result<String> {
    match bytes.len() <= OUTPUT_LIMIT {
        true => String::from_utf8(bytes.to_vec()).map_err(|_| {
            anyhow::anyhow!("The Git blob is binary; diff reports binary change metadata")
        }),
        false => Err(anyhow::anyhow!(
            "Git content exceeds the 32 MiB output limit"
        )),
    }
}

fn commit_info(commit: &git2::Commit<'_>) -> CommitInfo {
    CommitInfo {
        id: commit.id().to_string(),
        parents: commit.parent_ids().map(|id| id.to_string()).collect(),
        author: commit.author().name().unwrap_or_default().to_owned(),
        time: commit.time().seconds(),
        message: String::from_utf8_lossy(commit.message_bytes()).into_owned(),
    }
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

fn initialize() -> anyhow::Result<()> {
    static INITIALIZED: std::sync::OnceLock<Result<(), String>> = std::sync::OnceLock::new();
    INITIALIZED
        .get_or_init(|| {
            [
                git2::ConfigLevel::System,
                git2::ConfigLevel::Global,
                git2::ConfigLevel::XDG,
                git2::ConfigLevel::ProgramData,
            ]
            .into_iter()
            .try_for_each(|level| {
                unsafe { git2::opts::set_search_path(level, Path::new("/dev/null")) }
                    .map_err(|error| error.to_string())
            })
        })
        .as_ref()
        .map(|_| ())
        .map_err(|error| anyhow::anyhow!("Cannot disable external Git configuration: {error}"))
}
