use std::process::Output;
use tokio::process::Command;

use sandbox::ProcessLimits;
#[cfg(unix)]
mod workspace;

pub struct Sandbox;

pub trait SandboxOperation: sealed::Operation {}

pub(crate) mod sealed {
    pub trait Operation {
        fn into_command(self) -> tokio::process::Command;
    }
}

impl Sandbox {
    pub async fn capture(
        operation: impl SandboxOperation,
        timeout_seconds: u64,
    ) -> anyhow::Result<crate::process::ProcessOutput> {
        use crate::execution::ExecutionScope;
        use crate::process::ProcessHandle;
        let scope = ExecutionScope::current();
        let command = operation.into_command();
        let handle = ProcessHandle::new(
            crate::process::ProcessCommand::from_command(&command),
            scope.cancel.child_token(),
        );
        let _guard = handle.cancellation().drop_guard();
        Self::launch(command, timeout_seconds, handle.clone(), scope).await?;
        handle.wait().await;
        Ok(handle.output())
    }

    pub async fn start(
        operation: impl SandboxOperation,
        timeout_seconds: u64,
    ) -> anyhow::Result<String> {
        use crate::execution::ExecutionScope;
        use crate::process::ProcessHandle;
        let owner = ExecutionScope::current().process_owner();
        let command = operation.into_command();
        let handle = ProcessHandle::new(
            crate::process::ProcessCommand::from_command(&command),
            owner.cancel.child_token(),
        );
        let id = owner.processes.insert(handle.clone())?;
        let launched = Self::launch(command, timeout_seconds, handle.clone(), owner).await;
        if let Err(error) = &launched {
            handle.fail(format!("{error:#}"));
        }
        Ok(id)
    }

    async fn launch(
        command: Command,
        timeout_seconds: u64,
        handle: std::sync::Arc<crate::process::ProcessHandle>,
        owner: crate::execution::ExecutionScope,
    ) -> anyhow::Result<()> {
        let limits = ProcessLimits::new(
            std::time::Duration::from_secs(timeout_seconds),
            16 * 1024 * 1024,
        )?;
        #[cfg(unix)]
        {
            launch_command(command, limits, handle, owner).await
        }
        #[cfg(not(unix))]
        {
            let _ = (command, limits, handle, owner);
            Err(anyhow::anyhow!(
                "Managed process cleanup is unsupported on this platform"
            ))
        }
    }

    pub async fn output(operation: impl SandboxOperation) -> anyhow::Result<Output> {
        execute(operation.into_command(), ProcessLimits::default()).await
    }
}

#[cfg(unix)]
async fn launch_command(
    command: Command,
    limits: ProcessLimits,
    handle: std::sync::Arc<crate::process::ProcessHandle>,
    owner: crate::execution::ExecutionScope,
) -> anyhow::Result<()> {
    let scope = crate::execution::ExecutionScope::current();
    let policy = std::sync::Arc::new(workspace::SandboxWorkspace::new(scope.workspace()?));
    let sandbox = sandbox::Sandbox::new(policy, scope.tasks.clone());
    let process = sandbox
        .launch(
            command,
            limits,
            handle,
            vec![scope.cancel, owner.cancel.clone()],
        )
        .await?;
    let registration = owner.register(
        crate::execution::ResourceKind::Process,
        format!("Process {}", process.id()),
    );
    owner.tasks.spawn(process.run(move || drop(registration)));
    Ok(())
}

async fn execute(command: Command, limits: ProcessLimits) -> anyhow::Result<Output> {
    #[cfg(unix)]
    {
        use crate::execution::ExecutionScope;
        use crate::process::{ProcessHandle, ProcessStatus};
        use std::os::unix::process::ExitStatusExt;
        let scope = ExecutionScope::current();
        let handle = ProcessHandle::new(
            crate::process::ProcessCommand::from_command(&command),
            scope.cancel.child_token(),
        );
        let _guard = handle.cancellation().drop_guard();
        launch_command(command, limits, handle.clone(), scope).await?;
        handle.wait().await;
        let output = handle.output();
        match output.status {
            ProcessStatus::Exited => Ok(Output {
                status: std::process::ExitStatus::from_raw(
                    output
                        .exit_code
                        .map(|code| code << 8)
                        .unwrap_or(libc::SIGKILL),
                ),
                stdout: output.stdout.into_bytes(),
                stderr: output.stderr.into_bytes(),
            }),
            ProcessStatus::Cancelled => Err(anyhow::anyhow!("Process cancelled")),
            ProcessStatus::TimedOut => Err(anyhow::anyhow!("Process exceeded its time limit")),
            _ => Err(anyhow::anyhow!(
                "{}",
                output.error.unwrap_or_else(|| "Process failed".into())
            )),
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (command, limits);
        Err(anyhow::anyhow!(
            "Managed process cleanup is unsupported on this platform"
        ))
    }
}

#[cfg(all(test, any(target_os = "macos", target_os = "linux")))]
#[path = "../tests/unit/sandbox/tests.rs"]
mod tests;
