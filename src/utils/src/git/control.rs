use crate::workspace::{Access, WorkspacePolicy};
use std::path::{Path, PathBuf};

pub(super) struct RepositoryLayout {
    pub control: PathBuf,
    pub common: PathBuf,
}

impl RepositoryLayout {
    pub fn discover(workspace: &WorkspacePolicy) -> anyhow::Result<Option<Self>> {
        let workspace = workspace
            .permits_workspace_access(Access::Read)
            .then_some(workspace)
            .ok_or_else(|| {
                anyhow::anyhow!("Git and aggregate review require whole-project path access")
            })?;
        let dotgit = workspace.root().join(".git");
        match std::fs::symlink_metadata(&dotgit) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
            Ok(metadata) if metadata.is_dir() => Ok(Some(Self {
                control: dotgit.clone(),
                common: dotgit,
            })),
            Ok(metadata) if metadata.is_file() => Self::linked(workspace, &dotgit).map(Some),
            Ok(_) => Err(anyhow::anyhow!(
                "Git metadata must be an ordinary file or directory"
            )),
        }
    }

    fn linked(workspace: &WorkspacePolicy, dotgit: &Path) -> anyhow::Result<Self> {
        let text = workspace.read(dotgit)?;
        let path = text
            .trim()
            .strip_prefix("gitdir: ")
            .ok_or_else(|| anyhow::anyhow!("Invalid linked-worktree metadata"))?;
        let control = workspace.root().join(path).canonicalize()?;
        let common = control.join("../..").canonicalize()?;
        let backlink = ordinary_text(&control.join("gitdir"))?;
        let common_link = ordinary_text(&control.join("commondir"))?;
        let valid = control.parent().and_then(Path::file_name)
            == Some(std::ffi::OsStr::new("worktrees"))
            && common.file_name() == Some(std::ffi::OsStr::new(".git"))
            && Path::new(backlink.trim()).canonicalize()? == dotgit.canonicalize()?
            && control.join(common_link.trim()).canonicalize()? == common;
        match valid {
            true => Ok(Self { control, common }),
            false => Err(anyhow::anyhow!(
                "Linked worktree does not have matching Git control metadata"
            )),
        }
    }
}

pub(super) struct ControlDirectory {
    pub path: PathBuf,
}

impl ControlDirectory {
    pub fn new(root: PathBuf) -> anyhow::Result<Self> {
        let mut pending = vec![root.clone()];
        let mut visited = 0usize;
        while let Some(path) = pending.pop() {
            visited += 1;
            match ControlEntry::new(path, visited)? {
                ControlEntry::Directory { path } => pending.extend(
                    std::fs::read_dir(path)?
                        .map(|entry| entry.map(|entry| entry.path()))
                        .collect::<Result<Vec<_>, _>>()?,
                ),
                ControlEntry::File => {}
            }
        }
        Ok(Self { path: root })
    }
}

enum ControlEntry {
    Directory { path: PathBuf },
    File,
}

impl ControlEntry {
    fn new(path: PathBuf, visited: usize) -> anyhow::Result<Self> {
        let metadata = std::fs::symlink_metadata(&path)?;
        match metadata {
            metadata if metadata.is_symlink() || visited > 250_000 => Err(anyhow::anyhow!(
                "Git metadata has a symlink or exceeds 250000 entries"
            )),
            metadata if metadata.is_dir() => Ok(Self::Directory { path }),
            metadata if metadata.is_file() => Self::file(&path, &metadata),
            _ => Err(anyhow::anyhow!("Git metadata contains a special file")),
        }
    }

    fn file(path: &Path, metadata: &std::fs::Metadata) -> anyhow::Result<Self> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            match metadata.nlink() {
                1 => Ok(()),
                _ => Err(anyhow::anyhow!("Git metadata hard links are unsupported")),
            }?;
        }
        match ControlFile::from_path(path) {
            ControlFile::Configuration => {
                let includes = ordinary_text(path)?
                    .lines()
                    .map(str::trim)
                    .map(str::to_ascii_lowercase)
                    .any(|line| {
                        line.strip_prefix('[')
                            .is_some_and(|section| section.trim_start().starts_with("include"))
                    });
                match includes {
                    true => Err(anyhow::anyhow!(
                        "Git config includes are unavailable inside the fixed project boundary"
                    )),
                    false => Ok(Self::File),
                }
            }
            ControlFile::AlternateStore if metadata.len() != 0 => Err(anyhow::anyhow!(
                "External Git object stores are unavailable"
            )),
            ControlFile::AlternateStore | ControlFile::Ordinary => Ok(Self::File),
        }
    }
}

enum ControlFile {
    Configuration,
    AlternateStore,
    Ordinary,
}

impl ControlFile {
    fn from_path(path: &Path) -> Self {
        let normalized = path.to_string_lossy().to_ascii_lowercase();
        match path.file_name() {
            Some(name)
                if name.eq_ignore_ascii_case("config")
                    || name.eq_ignore_ascii_case("config.worktree") =>
            {
                Self::Configuration
            }
            _ if normalized.ends_with("/objects/info/alternates")
                || normalized.ends_with("/objects/info/http-alternates") =>
            {
                Self::AlternateStore
            }
            _ => Self::Ordinary,
        }
    }
}

fn ordinary_text(path: &Path) -> anyhow::Result<String> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Missing Git metadata parent"))?;
    WorkspacePolicy::workspace(parent.to_path_buf())?.read(path)
}

pub(super) fn initialize() -> anyhow::Result<()> {
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
