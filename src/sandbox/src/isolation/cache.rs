use crate::provision::directory::PrivateDirectory;
use std::{
    fs::{File, OpenOptions, TryLockError},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    time::Duration,
};
use tokio_util::sync::CancellationToken;

pub struct BuildCache {
    directory: PrivateDirectory,
    artifacts: PrivateDirectory,
}

enum LeaseState {
    Waiting,
    Acquired,
}

impl BuildCache {
    pub fn new(path: PathBuf, workspace: &Path) -> anyhow::Result<Self> {
        let directory = PrivateDirectory::new(path)?;
        match directory.path().starts_with(workspace) || workspace.starts_with(directory.path()) {
            true => Err(anyhow::anyhow!("Compiler cache overlaps the workspace")),
            false => {
                let artifacts = PrivateDirectory::new(directory.path().join("artifacts"))?;
                Ok(Self {
                    directory,
                    artifacts,
                })
            }
        }
    }

    pub fn path(&self) -> &Path {
        self.artifacts.path()
    }

    pub async fn lease(&self, cancellations: &[CancellationToken]) -> anyhow::Result<File> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(self.directory.path().join("lock"))?;
        let metadata = file.metadata()?;
        match metadata.is_file()
            && metadata.uid() == unsafe { libc::geteuid() }
            && metadata.nlink() == 1
            && metadata.mode() & 0o077 == 0
        {
            true => Ok(()),
            false => Err(anyhow::anyhow!("Compiler cache lock is not a private file")),
        }?;
        let mut state = LeaseState::Waiting;
        while matches!(state, LeaseState::Waiting) {
            match cancellations.iter().any(CancellationToken::is_cancelled) {
                true => Err(anyhow::anyhow!(
                    "Cancelled while waiting for the compiler cache"
                )),
                false => Ok(()),
            }?;
            state = match file.try_lock() {
                Ok(()) => LeaseState::Acquired,
                Err(TryLockError::WouldBlock) => {
                    tokio::time::sleep(Duration::from_millis(25)).await;
                    LeaseState::Waiting
                }
                Err(TryLockError::Error(error)) => Err(error)?,
            };
        }
        Ok(file)
    }
}

#[cfg(test)]
#[path = "../../tests/unit/isolation/cache/tests.rs"]
mod tests;
