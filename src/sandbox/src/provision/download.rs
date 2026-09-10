use super::{
    artifact::{ArchivePath, Artifact, Checksum},
    directory::PrivateDirectory,
};
use anyhow::Context;
use reqwest::blocking::Client;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    time::Duration,
};

enum LockState {
    Waiting,
    Acquired,
}

enum InstallationState {
    Ready,
    Pending,
}

pub struct Installation {
    directory: PrivateDirectory,
    _lock: File,
}

impl Installation {
    pub fn new(path: PathBuf, check: &dyn Fn() -> anyhow::Result<()>) -> anyhow::Result<Self> {
        let directory = PrivateDirectory::new(path)?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(directory.path().join("lock"))?;
        let mut state = LockState::Waiting;
        while matches!(state, LockState::Waiting) {
            check()?;
            state = match lock.try_lock() {
                Ok(()) => LockState::Acquired,
                Err(std::fs::TryLockError::WouldBlock) => {
                    std::thread::sleep(Duration::from_millis(100));
                    LockState::Waiting
                }
                Err(std::fs::TryLockError::Error(error)) => Err(error)?,
            };
        }
        Ok(Self {
            directory,
            _lock: lock,
        })
    }

    pub fn path(&self) -> &Path {
        self.directory.path()
    }

    pub fn prepare(
        &self,
        name: &str,
        install: impl FnOnce(&Path) -> anyhow::Result<()>,
    ) -> anyhow::Result<PathBuf> {
        let destination = self.path().join(name);
        let state = match destination.join("complete").is_file() {
            true => InstallationState::Ready,
            false => InstallationState::Pending,
        };
        match state {
            InstallationState::Ready => Ok(destination),
            InstallationState::Pending => {
                let staging = self.path().join(format!("{name}.partial"));
                for path in [&staging, &destination]
                    .into_iter()
                    .filter(|path| path.exists())
                {
                    fs::remove_dir_all(path)?;
                }
                let staging = PrivateDirectory::new(staging)?;
                install(staging.path())?;
                File::create(staging.path().join("complete"))?.sync_all()?;
                fs::rename(staging.path(), &destination)?;
                Ok(destination)
            }
        }
    }
}

enum DownloadState {
    Cached,
    Missing,
}

struct VerifiedDownload {
    path: PathBuf,
    file: File,
}

impl VerifiedDownload {
    fn receive(
        mut response: impl Read,
        path: PathBuf,
        artifact: &Artifact,
        check: &dyn Fn() -> anyhow::Result<()>,
    ) -> anyhow::Result<Self> {
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&path)?;
        let mut hash = Sha256::new();
        let mut remaining = 1024 * 1024 * 1024u64;
        let mut buffer = [0; 128 * 1024];
        let mut count = response.read(&mut buffer)?;
        while count > 0 {
            check()?;
            remaining = remaining
                .checked_sub(count as u64)
                .context("Sandbox component exceeds 1 GiB")?;
            hash.update(&buffer[..count]);
            file.write_all(&buffer[..count])?;
            count = response.read(&mut buffer)?;
        }
        match format!("{:x}", hash.finalize()) == artifact.checksum().as_str() {
            true => Ok(Self { path, file }),
            false => {
                fs::remove_file(path)?;
                Err(anyhow::anyhow!(
                    "Sandbox component checksum mismatch: {}",
                    artifact.url()
                ))
            }
        }
    }

    fn publish(self, destination: &Path) -> anyhow::Result<()> {
        self.file.sync_all()?;
        fs::rename(self.path, destination)?;
        Ok(())
    }
}

pub struct Downloads<'a> {
    directory: PrivateDirectory,
    client: Client,
    check: &'a dyn Fn() -> anyhow::Result<()>,
}

impl<'a> Downloads<'a> {
    pub fn new(path: PathBuf, check: &'a dyn Fn() -> anyhow::Result<()>) -> anyhow::Result<Self> {
        Ok(Self {
            directory: PrivateDirectory::new(path)?,
            client: Client::builder()
                .https_only(true)
                .connect_timeout(Duration::from_secs(30))
                .timeout(Duration::from_secs(60))
                .build()?,
            check,
        })
    }

    pub fn check(&self) -> anyhow::Result<()> {
        (self.check)()
    }

    pub fn json<T: serde::de::DeserializeOwned>(&self, url: &str) -> anyhow::Result<T> {
        Ok(serde_json::from_str(&self.text(url)?)?)
    }

    pub fn text(&self, url: &str) -> anyhow::Result<String> {
        self.check()?;
        let response = self.client.get(url).send()?.error_for_status()?;
        let mut text = String::new();
        response.take(16 * 1024 * 1024).read_to_string(&mut text)?;
        Ok(text)
    }

    pub fn get(&self, artifact: &Artifact, token: Option<&str>) -> anyhow::Result<PathBuf> {
        let destination = self.directory.path().join(artifact.checksum().as_str());
        self.check()?;
        let state =
            match destination.is_file() && &self.checksum(&destination)? == artifact.checksum() {
                true => DownloadState::Cached,
                false => DownloadState::Missing,
            };
        match state {
            DownloadState::Cached => Ok(destination),
            DownloadState::Missing => {
                let request = self.client.get(artifact.url().clone()).header("Accept", "application/vnd.oci.image.manifest.v1+json, application/vnd.docker.distribution.manifest.v2+json");
                let request = match token {
                    Some(token) => request.bearer_auth(token),
                    None => request,
                };
                let response = request.send()?.error_for_status()?;
                let partial = destination.with_extension("partial");
                VerifiedDownload::receive(response, partial, artifact, self.check)?
                    .publish(&destination)?;
                Ok(destination)
            }
        }
    }

    fn checksum(&self, path: &Path) -> anyhow::Result<Checksum> {
        let mut file = File::open(path)?;
        let mut hash = Sha256::new();
        let mut buffer = [0; 128 * 1024];
        let mut count = file.read(&mut buffer)?;
        while count > 0 {
            self.check()?;
            hash.update(&buffer[..count]);
            count = file.read(&mut buffer)?;
        }
        Checksum::new(&format!("{:x}", hash.finalize()))
    }

    pub fn unpack(&self, archive: &Path, destination: &Path) -> anyhow::Result<()> {
        fs::create_dir_all(destination)?;
        let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(File::open(archive)?));
        archive.set_preserve_permissions(false);
        archive.set_preserve_ownerships(false);
        let mut remaining = 8 * 1024 * 1024 * 1024u64;
        for entry in archive.entries()? {
            self.check()?;
            let mut entry = entry?;
            let path = ArchivePath::new(entry.path()?.into_owned())?;
            remaining = remaining
                .checked_sub(entry.size())
                .context("Sandbox archive exceeds 8 GiB")?;
            match entry.header().entry_type() {
                tar::EntryType::Regular
                | tar::EntryType::Directory
                | tar::EntryType::Symlink
                | tar::EntryType::Link => {
                    match entry
                        .unpack_in(destination)
                        .with_context(|| format!("Cannot extract {}", path.path().display()))?
                    {
                        true => Ok(()),
                        false => Err(anyhow::anyhow!(
                            "Archive entry escapes its sandbox directory"
                        )),
                    }?;
                }
                _ => {}
            }
        }
        Ok(())
    }
}
