use std::process::Output;
use tokio::process::Command;

#[cfg(unix)]
mod isolation;

#[cfg(any(target_os = "linux", target_os = "macos"))]
mod protocol;

struct ProcessLimits {
    timeout: std::time::Duration,
    output_bytes: u64,
}

impl ProcessLimits {
    fn new(timeout: std::time::Duration, output_bytes: u64) -> anyhow::Result<Self> {
        if timeout.is_zero()
            || timeout > std::time::Duration::from_secs(300)
            || output_bytes == 0
            || output_bytes > 16 * 1024 * 1024
        {
            Err(anyhow::anyhow!(
                "Process limits must fit within five minutes and 16 MiB per stream"
            ))
        } else {
            Ok(Self {
                timeout,
                output_bytes,
            })
        }
    }
}

impl Default for ProcessLimits {
    fn default() -> Self {
        Self {
            timeout: std::time::Duration::from_secs(300),
            output_bytes: 16 * 1024 * 1024,
        }
    }
}

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
        let _guard = handle.cancel.clone().drop_guard();
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
            handle.complete(
                crate::process::ProcessEnd::Failed {
                    error: format!("{error:#}"),
                },
                None,
            );
            handle.done.cancel();
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
            unix::launch(command, limits, handle, owner).await
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

async fn execute(command: Command, limits: ProcessLimits) -> anyhow::Result<Output> {
    #[cfg(unix)]
    {
        unix::output(command, limits).await
    }
    #[cfg(not(unix))]
    {
        let _ = command;
        let _ = limits;
        Err(anyhow::anyhow!(
            "Managed process cleanup is unsupported on this platform"
        ))
    }
}

#[cfg(unix)]
mod unix {
    use super::isolation::{IsolatedCommand, TemporaryDirectory};
    use super::*;
    use crate::execution::{ExecutionScope, ResourceKind};
    use crate::process::{OutputStream, ProcessEnd, ProcessHandle, ProcessStatus};
    use std::{process::Stdio, sync::Arc};
    use tokio::{
        io::AsyncReadExt,
        process::{Child, ChildStderr, ChildStdout},
    };

    struct ProcessGroup {
        leader: u32,
    }
    impl ProcessGroup {
        fn kill(&self) {
            unsafe {
                libc::kill(-(self.leader as i32), libc::SIGKILL);
            }
        }
    }

    struct RunningProcess {
        child: Child,
        group: ProcessGroup,
        stdout: ChildStdout,
        stderr: ChildStderr,
        temporary: TemporaryDirectory,
    }

    impl RunningProcess {
        async fn spawn(mut prepared: IsolatedCommand) -> anyhow::Result<Self> {
            prepared
                .command
                .process_group(0)
                .kill_on_drop(true)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            let mut child = prepared
                .command
                .spawn()
                .map_err(|error| anyhow::anyhow!("Cannot launch sandbox executable: {error}"))?;
            match (child.id(), child.stdout.take(), child.stderr.take()) {
                (Some(pid), Some(stdout), Some(stderr)) => Ok(Self {
                    child,
                    group: ProcessGroup { leader: pid },
                    stdout,
                    stderr,
                    temporary: prepared.temporary,
                }),
                _ => {
                    let _ = child.start_kill();
                    let _ = child.wait().await;
                    Err(anyhow::anyhow!(
                        "Spawned process is missing its ID or pipes"
                    ))
                }
            }
        }

        async fn run(self, handle: Arc<ProcessHandle>, limits: ProcessLimits) {
            let Self {
                mut child,
                group,
                stdout,
                stderr,
                temporary,
            } = self;
            let stdout = read_output(
                stdout,
                handle.clone(),
                OutputStream::Stdout,
                limits.output_bytes as usize,
            );
            let stderr = read_output(
                stderr,
                handle.clone(),
                OutputStream::Stderr,
                limits.output_bytes as usize,
            );
            tokio::pin!(stdout, stderr);
            let completion = async {
                let wait = async {
                    let status = child.wait().await;
                    group.kill();
                    status.map_err(OutputFailure::Io)
                };
                tokio::try_join!(wait, &mut stdout, &mut stderr).map(|(status, _, _)| status)
            };
            let result = tokio::select! {
                biased;
                _ = handle.cancel.cancelled() => Completion::Cancelled,
                _ = tokio::time::sleep(limits.timeout) => Completion::TimedOut,
                result = completion => match result {
                    Ok(status) => Completion::Exited(status),
                    Err(error) => Completion::Failed(error),
                },
            };
            group.kill();
            let exit_code = match &result {
                Completion::Exited(status) => status.code(),
                _ => {
                    let _ = child.start_kill();
                    child.wait().await.ok().and_then(|status| status.code())
                }
            };
            let end = match result {
                Completion::Exited(_) => ProcessEnd::Exited,
                Completion::Cancelled => ProcessEnd::Cancelled,
                Completion::TimedOut => ProcessEnd::TimedOut,
                Completion::Failed(OutputFailure::Limit) => ProcessEnd::OutputLimit,
                Completion::Failed(OutputFailure::Io(error)) => ProcessEnd::Failed {
                    error: error.to_string(),
                },
            };
            drop(temporary);
            handle.complete(end, exit_code);
        }
    }

    enum Completion {
        Exited(std::process::ExitStatus),
        Cancelled,
        TimedOut,
        Failed(OutputFailure),
    }

    enum OutputFailure {
        Limit,
        Io(std::io::Error),
    }

    impl From<std::io::Error> for OutputFailure {
        fn from(error: std::io::Error) -> Self {
            Self::Io(error)
        }
    }

    async fn read_output(
        mut reader: impl tokio::io::AsyncRead + Unpin,
        handle: Arc<ProcessHandle>,
        stream: OutputStream,
        limit: usize,
    ) -> Result<(), OutputFailure> {
        let mut buffer = [0; 8192];
        let mut count = reader.read(&mut buffer).await?;
        while count > 0 {
            handle
                .append(stream, &buffer[..count], limit)
                .then_some(())
                .ok_or(OutputFailure::Limit)?;
            count = reader.read(&mut buffer).await?;
        }
        Ok(())
    }

    pub(super) async fn launch(
        command: Command,
        limits: ProcessLimits,
        handle: Arc<ProcessHandle>,
        owner: ExecutionScope,
    ) -> anyhow::Result<()> {
        let scope = ExecutionScope::current();
        let workspace = scope.workspace()?;
        let cancellations = [
            scope.cancel.clone(),
            owner.cancel.clone(),
            handle.cancel.clone(),
        ];
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        if command.as_std().get_program() == "cargo"
            && command.as_std().get_args().next() != Some(std::ffi::OsStr::new("fmt"))
        {
            super::isolation::registry::prepare(workspace.clone(), cancellations.to_vec()).await?;
        }
        let prepared = scope
            .tasks
            .spawn_blocking(move || {
                IsolatedCommand::new(command, &workspace, &|| match cancellations
                    .iter()
                    .any(|cancel| cancel.is_cancelled())
                {
                    true => Err(anyhow::anyhow!(
                        "Process cancelled before launch during sandbox preparation"
                    )),
                    false => Ok(()),
                })
            })
            .await??;
        match scope.cancel.is_cancelled()
            || owner.cancel.is_cancelled()
            || handle.cancel.is_cancelled()
        {
            true => Err(anyhow::anyhow!("Process cancelled before launch")),
            false => {
                let process = RunningProcess::spawn(prepared).await?;
                let registration = owner.register(
                    ResourceKind::Process,
                    format!("Process {}", process.group.leader),
                );
                owner.tasks.spawn(async move {
                    process.run(handle.clone(), limits).await;
                    drop(registration);
                    handle.done.cancel();
                });
                Ok(())
            }
        }
    }

    pub(super) async fn output(command: Command, limits: ProcessLimits) -> anyhow::Result<Output> {
        use std::os::unix::process::ExitStatusExt;
        let scope = ExecutionScope::current();
        let handle = ProcessHandle::new(
            crate::process::ProcessCommand::from_command(&command),
            scope.cancel.child_token(),
        );
        let _guard = handle.cancel.clone().drop_guard();
        launch(command, limits, handle.clone(), scope).await?;
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
}

#[cfg(all(test, any(target_os = "macos", target_os = "linux")))]
mod tests;
