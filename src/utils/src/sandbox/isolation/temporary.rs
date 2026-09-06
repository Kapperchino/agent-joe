use crate::workspace::WorkspacePolicy;
use std::{
    os::unix::fs::DirBuilderExt,
    path::{Path, PathBuf},
};

pub(crate) struct TemporaryDirectory {
    path: PathBuf,
}

impl TemporaryDirectory {
    pub(super) fn new(workspace: &WorkspacePolicy) -> anyhow::Result<Self> {
        workspace.create_parent_dirs(Path::new("target/.joe/tmp/placeholder"))?;
        let path = workspace
            .root()
            .join("target/.joe/tmp")
            .join(uuid::Uuid::new_v4().to_string());
        std::fs::DirBuilder::new().mode(0o700).create(&path)?;
        Ok(Self { path })
    }

    pub(super) fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TemporaryDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}
