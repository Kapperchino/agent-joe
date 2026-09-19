use crate::workspace::Workspace;
use anyhow::Context;
use rustix::fs::{self, AtFlags, FileType, Mode, OFlags};
use std::{
    os::fd::OwnedFd,
    path::{Component, Path, PathBuf},
};

pub struct TemporaryDirectory {
    path: PathBuf,
    directory: OwnedFd,
    parent: OwnedFd,
    id: uuid::Uuid,
}

impl TemporaryDirectory {
    pub(super) fn new(workspace: &dyn Workspace) -> anyhow::Result<Self> {
        workspace.create_parent_dirs(Path::new("target/.joe/tmp/placeholder"))?;
        let path = workspace.root().join("target/.joe/tmp");
        Self::create(open_directory(&path)?, &path)
    }

    fn create(parent: OwnedFd, path: &Path) -> anyhow::Result<Self> {
        let id = uuid::Uuid::new_v4();
        let name = id.to_string();
        fs::mkdirat(&parent, &name, Mode::from_raw_mode(0o700))?;
        let directory = fs::openat(&parent, &name, directory_flags(), Mode::empty())?;
        Ok(Self {
            path: path.join(name),
            directory,
            parent,
            id,
        })
    }

    pub fn id(&self) -> uuid::Uuid {
        self.id
    }

    pub fn child(&self) -> anyhow::Result<Self> {
        let child = Self::create(self.reopen()?, &self.path)?;
        fs::mkdirat(&child.directory, "guest", Mode::from_raw_mode(0o700))?;
        Ok(child)
    }

    fn reopen(&self) -> anyhow::Result<OwnedFd> {
        let directory = open_directory(&self.path)?;
        let pinned = fs::fstat(&self.directory)?;
        let current = fs::fstat(&directory)?;
        match pinned.st_dev == current.st_dev && pinned.st_ino == current.st_ino {
            true => Ok(directory),
            false => Err(anyhow::anyhow!(
                "Sandbox temporary directory changed: {}",
                self.path.display()
            )),
        }
    }

    pub fn remove(&self) {
        let _ = self.reopen().and_then(|directory| {
            remove_contents(&directory)?;
            let current = fs::statat(&self.parent, self.id.to_string(), AtFlags::SYMLINK_NOFOLLOW)?;
            let pinned = fs::fstat(&directory)?;
            match current.st_dev == pinned.st_dev && current.st_ino == pinned.st_ino {
                true => Ok(fs::unlinkat(
                    &self.parent,
                    self.id.to_string(),
                    AtFlags::REMOVEDIR,
                )?),
                false => Err(anyhow::anyhow!(
                    "Sandbox temporary directory changed during cleanup"
                )),
            }
        });
    }

    pub(super) fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TemporaryDirectory {
    fn drop(&mut self) {
        self.remove();
    }
}

fn directory_flags() -> OFlags {
    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW
}

fn open_directory(path: &Path) -> anyhow::Result<OwnedFd> {
    let root = match path.is_absolute() {
        true => fs::open("/", directory_flags(), Mode::empty())?,
        false => Err(anyhow::anyhow!("Sandbox directories must be absolute"))?,
    };
    path.components()
        .try_fold(root, |parent, component| match component {
            Component::RootDir => Ok(parent),
            Component::Normal(name) => fs::openat(&parent, name, directory_flags(), Mode::empty())
                .with_context(|| {
                    format!(
                        "Cannot open sandbox directory without following links: {}",
                        path.display()
                    )
                }),
            _ => Err(anyhow::anyhow!(
                "An absolute sandbox directory without traversal is required"
            )),
        })
}

fn remove_contents(directory: &OwnedFd) -> anyhow::Result<()> {
    fs::Dir::read_from(directory)?
        .filter_map(|entry| match entry {
            Ok(entry) if matches!(entry.file_name().to_bytes(), b"." | b"..") => None,
            entry => Some(entry),
        })
        .try_for_each(|entry| {
            let entry = entry?;
            let name = entry.file_name();
            let metadata = fs::statat(directory, name, AtFlags::SYMLINK_NOFOLLOW)?;
            match FileType::from_raw_mode(metadata.st_mode) {
                FileType::Directory => {
                    let child = fs::openat(directory, name, directory_flags(), Mode::empty())?;
                    let pinned = fs::fstat(&child)?;
                    match metadata.st_dev == pinned.st_dev && metadata.st_ino == pinned.st_ino {
                        true => {
                            remove_contents(&child)?;
                            fs::unlinkat(directory, name, AtFlags::REMOVEDIR)?;
                            Ok(())
                        }
                        false => Err(anyhow::anyhow!(
                            "Sandbox temporary entry changed during cleanup"
                        )),
                    }
                }
                _ => Ok(fs::unlinkat(directory, name, AtFlags::empty())?),
            }
        })
}

#[cfg(test)]
#[path = "../../tests/unit/isolation/temporary/tests.rs"]
mod tests;
