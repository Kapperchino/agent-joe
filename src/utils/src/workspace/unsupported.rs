use super::*;

impl WorkspacePolicy {
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
