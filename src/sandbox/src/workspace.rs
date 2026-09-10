use std::path::{Path, PathBuf};

#[derive(Default)]
pub struct WorkspaceProtection {
    pub read_only: Vec<PathBuf>,
    pub hidden: Vec<PathBuf>,
}

pub trait Workspace: Send + Sync {
    fn root(&self) -> &Path;
    fn prepare(&self) -> anyhow::Result<WorkspaceProtection>;
    fn read(&self, path: &Path) -> anyhow::Result<String>;
    fn create_parent_dirs(&self, path: &Path) -> anyhow::Result<()>;
    fn link_process_cache(&self, source: &Path, destination: &Path) -> anyhow::Result<()>;
}
