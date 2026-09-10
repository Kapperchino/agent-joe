use crate::workspace::{ProcessWorkspace, WorkspacePolicy};
use anyhow::Context;
use tokio::process::Command;

mod temporary;
pub(crate) use temporary::TemporaryDirectory;

#[cfg(any(target_os = "macos", target_os = "linux"))]
mod bootstrap;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(any(target_os = "macos", target_os = "linux"))]
#[path = "../../../../sandbox/provision/mod.rs"]
mod provision;
#[cfg(any(target_os = "macos", target_os = "linux"))]
pub(super) mod registry;
#[cfg(any(target_os = "macos", target_os = "linux"))]
mod runtime;
#[cfg(target_os = "linux")]
mod seccomp;

pub(super) struct IsolatedCommand {
    pub(super) command: Command,
    pub(super) temporary: TemporaryDirectory,
    #[cfg(target_os = "linux")]
    _filter: seccomp::Filter,
}

impl IsolatedCommand {
    pub(super) fn new(
        command: Command,
        policy: &WorkspacePolicy,
        check: &dyn Fn() -> anyhow::Result<()>,
    ) -> anyhow::Result<Self> {
        let policy = match policy.permits_workspace_execution() {
            true => Ok(policy),
            false => Err(anyhow::anyhow!(
                "Executable operations require worker access to the whole workspace"
            )),
        }?;
        let workspace =
            ProcessWorkspace::new(policy).context("Cannot prepare the process workspace")?;
        let temporary = TemporaryDirectory::new(policy)
            .context("Cannot create the process temporary directory")?;
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        {
            let runtime = runtime::Runtime::new(policy, check)?;
            let command = runtime.prepare(command, &workspace, &temporary)?;
            #[cfg(target_os = "macos")]
            let command = macos::prepare(command, policy, &runtime, &temporary)?;
            #[cfg(target_os = "linux")]
            let filter = seccomp::Filter::new()?;
            #[cfg(target_os = "linux")]
            let command = linux::prepare(command, policy, &runtime, &filter)?;
            Ok(Self {
                command,
                temporary,
                #[cfg(target_os = "linux")]
                _filter: filter,
            })
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        {
            let _ = (command, workspace, temporary);
            Err(anyhow::anyhow!(
                "Project process isolation is unavailable on this platform; execution is disabled"
            ))
        }
    }
}
