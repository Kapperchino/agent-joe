use crate::{
    changes::{Baseline, FileVersion},
    git::{GitRepository, excluded},
    workspace::{Access, DirectoryEntry, WorkspacePolicy},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

#[derive(Debug)]
pub(super) struct WorktreeSnapshot {
    pub files: BTreeMap<PathBuf, FileVersion>,
    pub head: String,
}

impl WorktreeSnapshot {
    pub fn with_git_modes(self) -> Self {
        Self {
            files: self
                .files
                .into_iter()
                .map(|(path, version)| (path, version.with_git_mode()))
                .collect(),
            head: self.head,
        }
    }

    pub fn base(git: &GitRepository, head: &str) -> anyhow::Result<Self> {
        let tree = git.repo.find_commit(git2::Oid::from_str(head)?)?.tree()?;
        let contents = SnapshotFiles::default().collect_tree(&git.repo, &tree, Path::new(""))?;
        Ok(Self {
            files: contents.files,
            head: head.to_owned(),
        })
    }

    pub fn current(workspace: &WorkspacePolicy, git: &GitRepository) -> anyhow::Result<Self> {
        Ok(Self {
            files: Baseline::capture(workspace)?
                .files
                .into_iter()
                .filter(|(_, version)| *version != FileVersion::Missing)
                .collect(),
            head: git
                .head()?
                .ok_or_else(|| anyhow::anyhow!("Worktree HEAD is missing"))?,
        })
    }

    pub fn for_cleanup(
        workspace: &WorkspacePolicy,
        git: &GitRepository,
        expected: &Self,
    ) -> anyhow::Result<Self> {
        let mut scan = DirectoryScan {
            pending: vec![workspace.root().to_path_buf()],
            contents: SnapshotFiles::default(),
            visited: 0,
        };
        while let Some(directory) = scan.pending.pop() {
            scan = workspace
                .entries(&directory)?
                .into_iter()
                .try_fold(scan, |scan, entry| scan.visit(workspace, expected, entry))?;
        }
        Ok(Self {
            files: scan.contents.files,
            head: git
                .head()?
                .ok_or_else(|| anyhow::anyhow!("Worktree HEAD is missing"))?,
        })
    }

    pub fn for_session_cleanup(
        workspace: &WorkspacePolicy,
        git: &GitRepository,
        expected: &Self,
    ) -> anyhow::Result<Self> {
        let contents = expected.files.iter().try_fold(
            SnapshotFiles::default(),
            |files, (path, version)| match workspace.file_size(path)?
                == version.bytes().len() as u64
            {
                true => files.with_file(path.clone(), workspace.file_version(path)?),
                false => Err(anyhow::anyhow!(
                    "Cleanup conflict: file size changed: {}. Preserve or restore this local change before retrying",
                    path.display()
                )),
            },
        )?;
        Ok(Self {
            files: contents.files,
            head: git
                .head()?
                .ok_or_else(|| anyhow::anyhow!("Worktree HEAD is missing"))?,
        })
    }
}

#[derive(Default)]
struct SnapshotFiles {
    files: BTreeMap<PathBuf, FileVersion>,
    bytes: usize,
}

impl SnapshotFiles {
    fn with_file(mut self, path: PathBuf, version: FileVersion) -> anyhow::Result<Self> {
        let bytes = self.bytes.saturating_add(version.bytes().len());
        let fits = bytes <= 64 * 1024 * 1024
            && version.bytes().len() <= 16 * 1024 * 1024
            && self.files.len() < 250_000;
        match fits {
            true => {
                self.files.insert(path, version);
                self.bytes = bytes;
                Ok(self)
            }
            false => Err(anyhow::anyhow!(
                "Git snapshot exceeds file, content, or entry limits"
            )),
        }
    }

    fn collect_tree(
        self,
        repo: &git2::Repository,
        tree: &git2::Tree<'_>,
        root: &Path,
    ) -> anyhow::Result<Self> {
        tree.iter().try_fold(self, |files, entry| {
            let path = root.join(entry.name()?);
            match entry.filemode() {
                0o040000 => files.collect_tree(repo, &repo.find_tree(entry.id())?, &path),
                0o100644 | 0o100755 => files.with_blob(repo, &entry, path),
                _ => Err(anyhow::anyhow!(
                    "Managed checkout requires ordinary files; symlinks and submodules are unsupported: {}",
                    path.display()
                )),
            }
        })
    }

    fn with_blob(
        self,
        repo: &git2::Repository,
        entry: &git2::TreeEntry<'_>,
        path: PathBuf,
    ) -> anyhow::Result<Self> {
        let size = repo.odb()?.read_header(entry.id())?.0;
        let fits = !excluded(&path)
            && size <= 16 * 1024 * 1024
            && self.bytes.saturating_add(size) <= 64 * 1024 * 1024
            && self.files.len() < 250_000;
        match fits {
            true => {
                let blob = repo.find_blob(entry.id())?;
                self.with_file(
                    path,
                    FileVersion::File {
                        content: blob.content().to_vec(),
                        mode: (entry.filemode() & 0o777) as u32,
                    },
                )
            }
            false => Err(anyhow::anyhow!(
                "Selected tree contains a protected path or exceeds snapshot limits"
            )),
        }
    }
}

struct DirectoryScan {
    pending: Vec<PathBuf>,
    contents: SnapshotFiles,
    visited: usize,
}

impl DirectoryScan {
    fn visit(
        mut self,
        workspace: &WorkspacePolicy,
        expected: &WorktreeSnapshot,
        entry: DirectoryEntry,
    ) -> anyhow::Result<Self> {
        self.visited += 1;
        match entry.path {
            _ if self.visited > 250_000 => {
                Err(anyhow::anyhow!("Worktree cleanup exceeds the entry limit"))
            }
            path if path == workspace.root().join(".git") => Ok(self),
            path if workspace.is_directory(&path)? => {
                self.pending.push(path);
                Ok(self)
            }
            path => {
                let relative = workspace.relative_path(&path, Access::Read)?;
                match expected.files.get(&relative) {
                    Some(version)
                        if workspace.file_size(&path)? == version.bytes().len() as u64 =>
                    {
                        self.contents = self
                            .contents
                            .with_file(relative, workspace.file_version(&path)?)?;
                        Ok(self)
                    }
                    Some(_) => Err(anyhow::anyhow!(
                        "Cleanup conflict: file size changed: {}. Preserve or restore this local change before retrying",
                        relative.display()
                    )),
                    None => Err(anyhow::anyhow!(
                        "Cleanup conflict: unrecorded file: {}. Move it out of the worktree or remove it before retrying",
                        relative.display()
                    )),
                }
            }
        }
    }
}

pub(super) fn changed_paths(
    before: &BTreeMap<PathBuf, FileVersion>,
    after: &BTreeMap<PathBuf, FileVersion>,
) -> BTreeSet<PathBuf> {
    before
        .keys()
        .chain(after.keys())
        .filter(|path| before.get(*path) != after.get(*path))
        .cloned()
        .collect()
}
