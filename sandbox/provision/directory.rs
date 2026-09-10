use std::{
    fs,
    os::unix::fs::{DirBuilderExt, MetadataExt},
    path::{Path, PathBuf},
};

pub struct PrivateDirectory {
    path: PathBuf,
}

impl PrivateDirectory {
    pub fn new(path: PathBuf) -> anyhow::Result<Self> {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&path)?;
        let metadata = fs::symlink_metadata(&path)?;
        match metadata.is_dir()
            && metadata.uid() == unsafe { libc::geteuid() }
            && metadata.mode() & 0o077 == 0
        {
            true => Ok(Self {
                path: path.canonicalize()?,
            }),
            false => Err(anyhow::anyhow!(
                "Sandbox cache is not a private directory: {}",
                path.display()
            )),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}
