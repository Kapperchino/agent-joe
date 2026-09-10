use crate::workspace::WorkspacePolicy;
use std::{
    os::unix::fs::DirBuilderExt,
    path::{Path, PathBuf},
};

pub(crate) struct TemporaryDirectory {
    path: PathBuf,
    id: uuid::Uuid,
}

impl TemporaryDirectory {
    pub(super) fn new(workspace: &WorkspacePolicy) -> anyhow::Result<Self> {
        workspace.create_parent_dirs(Path::new("target/.joe/tmp/placeholder"))?;
        let id = uuid::Uuid::new_v4();
        let path = workspace
            .root()
            .join("target/.joe/tmp")
            .join(id.to_string());
        std::fs::DirBuilder::new().mode(0o700).create(&path)?;
        Ok(Self { path, id })
    }

    pub(super) fn id(&self) -> uuid::Uuid {
        self.id
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
