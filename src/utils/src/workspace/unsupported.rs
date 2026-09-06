use super::*;

pub enum PrivateStorage {}

impl PrivateStorage {
    pub fn workspace_identity(&self) -> &str {
        match *self {}
    }
    pub fn new_id(&self) -> String {
        match *self {}
    }
    pub fn path(&self) -> &Path {
        match *self {}
    }
}

impl WorkspacePolicy {
    pub fn workspace_identity(&self) -> anyhow::Result<String> {
        Err(anyhow::anyhow!(
            "Protected session storage is unsupported on this platform"
        ))
    }
    pub fn session_storage(&self, _: &str) -> anyhow::Result<PrivateStorage> {
        Err(anyhow::anyhow!(
            "Protected session storage is unsupported on this platform"
        ))
    }
    pub fn open_append(&self, _: &Path) -> anyhow::Result<File> {
        Err(anyhow::anyhow!(
            "Descriptor-based workspace access is unsupported on this platform"
        ))
    }

    pub fn read(&self, _: &Path) -> anyhow::Result<String> {
        Err(anyhow::anyhow!(
            "Descriptor-based workspace access is unsupported on this platform"
        ))
    }

    pub fn is_directory(&self, _: &Path) -> anyhow::Result<bool> {
        Err(anyhow::anyhow!(
            "Descriptor-based workspace access is unsupported on this platform"
        ))
    }

    pub fn entries(&self, _: &Path) -> anyhow::Result<Vec<DirectoryEntry>> {
        Err(anyhow::anyhow!(
            "Descriptor-based workspace access is unsupported on this platform"
        ))
    }

    pub fn create_parent_dirs(&self, _: &Path) -> anyhow::Result<()> {
        Err(anyhow::anyhow!(
            "Descriptor-based workspace access is unsupported on this platform"
        ))
    }

    pub fn write(&self, _: &Path, _: &str) -> anyhow::Result<()> {
        Err(anyhow::anyhow!(
            "Descriptor-based workspace access is unsupported on this platform"
        ))
    }

    pub fn copy(&self, _: &Path, _: &Path) -> anyhow::Result<()> {
        Err(anyhow::anyhow!(
            "Descriptor-based workspace access is unsupported on this platform"
        ))
    }

    pub fn delete(&self, _: &Path) -> anyhow::Result<()> {
        Err(anyhow::anyhow!(
            "Descriptor-based workspace access is unsupported on this platform"
        ))
    }

    pub fn rename(&self, _: &Path, _: &Path) -> anyhow::Result<()> {
        Err(anyhow::anyhow!(
            "Descriptor-based workspace access is unsupported on this platform"
        ))
    }
}
