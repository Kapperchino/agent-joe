use crate::workspace::{ProcessWorkspace, WorkspacePolicy};
use sandbox::workspace::{Workspace, WorkspaceProtection};
use std::{path::Path, sync::Arc};

pub(super) struct SandboxWorkspace {
    policy: Arc<WorkspacePolicy>,
}

impl SandboxWorkspace {
    pub(super) fn new(policy: Arc<WorkspacePolicy>) -> Self {
        Self { policy }
    }
}

impl Workspace for SandboxWorkspace {
    fn root(&self) -> &Path {
        self.policy.root()
    }

    fn prepare(&self) -> anyhow::Result<WorkspaceProtection> {
        match self.policy.permits_workspace_execution() {
            true => {
                let policy = ProcessWorkspace::new(&self.policy)?.policy();
                let protection = WorkspaceProtection {
                    read_only: policy.read_only_roots().map(Path::to_path_buf).collect(),
                    hidden: Vec::new(),
                };
                #[cfg(target_os = "linux")]
                let protection = policy.process_protected_paths()?.into_iter().fold(
                    protection,
                    |mut protection, path| {
                        match policy.check(&path, crate::workspace::Access::Read).is_ok() {
                            true => protection.read_only.push(path),
                            false => protection.hidden.push(path),
                        }
                        protection
                    },
                );
                Ok(protection)
            }
            false => Err(anyhow::anyhow!(
                "Executable operations require worker access to the whole workspace"
            )),
        }
    }

    fn read(&self, path: &Path) -> anyhow::Result<String> {
        self.policy.read(path)
    }

    fn create_parent_dirs(&self, path: &Path) -> anyhow::Result<()> {
        self.policy.create_parent_dirs(path)
    }

    fn link_process_cache(&self, source: &Path, destination: &Path) -> anyhow::Result<()> {
        self.policy.link_process_cache(source, destination)
    }
}
