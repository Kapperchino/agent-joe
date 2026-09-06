use std::process::Output;
use tokio::process::Command;

#[cfg(unix)]
mod isolation;

#[cfg(target_os = "linux")]
mod seccomp;

struct ProcessLimits {
    timeout: std::time::Duration,
    output_bytes: u64,
}

impl ProcessLimits {
    #[cfg(test)]
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
    use std::process::Stdio;
    use tokio::{
        io::AsyncReadExt,
        process::{Child, ChildStderr, ChildStdout},
    };
    use tokio_util::sync::CancellationToken;

    struct ProcessGroup(u32);
    impl ProcessGroup {
        fn kill(&self) {
            unsafe {
                libc::kill(-(self.0 as i32), libc::SIGKILL);
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
            match prepared.command.spawn() {
                Ok(mut child) => match (child.id(), child.stdout.take(), child.stderr.take()) {
                    (Some(pid), Some(stdout), Some(stderr)) => Ok(Self {
                        child,
                        group: ProcessGroup(pid),
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
                },
                Err(error) => Err(error.into()),
            }
        }

        async fn run(
            self,
            cancel: CancellationToken,
            limits: ProcessLimits,
        ) -> anyhow::Result<Output> {
            let Self {
                mut child,
                group,
                stdout,
                stderr,
                temporary: _temporary,
            } = self;
            let completion = async {
                let read = async {
                    tokio::try_join!(
                        read_output(stdout, limits.output_bytes),
                        read_output(stderr, limits.output_bytes)
                    )
                };
                let wait = async {
                    let status = child.wait().await.map_err(anyhow::Error::from);
                    group.kill();
                    status
                };
                tokio::try_join!(wait, read)
                    .map(|(status, (stdout, stderr))| Output {
                        status,
                        stdout,
                        stderr,
                    })
                    .map_err(anyhow::Error::from)
            };
            let result = tokio::select! {
                biased;
                _ = cancel.cancelled() => Err(anyhow::anyhow!("Process cancelled")),
                _ = tokio::time::sleep(limits.timeout) => Err(anyhow::anyhow!("Process exceeded its time limit")),
                result = completion => result,
            };
            if result.is_err() {
                group.kill();
                let _ = child.start_kill();
                let _ = child.wait().await;
            }
            result
        }
    }

    async fn read_output(
        reader: impl tokio::io::AsyncRead + Unpin,
        limit: u64,
    ) -> anyhow::Result<Vec<u8>> {
        let mut content = Vec::new();
        reader.take(limit + 1).read_to_end(&mut content).await?;
        if content.len() as u64 > limit {
            Err(anyhow::anyhow!("Process output exceeded its stream limit"))
        } else {
            Ok(content)
        }
    }

    pub(super) async fn output(command: Command, limits: ProcessLimits) -> anyhow::Result<Output> {
        let scope = ExecutionScope::current();
        if scope.cancel.is_cancelled() {
            Err(anyhow::anyhow!("Process cancelled before launch"))
        } else {
            let workspace = scope.workspace()?;
            let cancel = scope.cancel.child_token();
            let _guard = cancel.clone().drop_guard();
            let command = scope
                .tasks
                .spawn_blocking(move || IsolatedCommand::new(command, &workspace))
                .await??;
            let launch = if cancel.is_cancelled() {
                Err(anyhow::anyhow!("Process cancelled before launch"))
            } else {
                RunningProcess::spawn(command).await
            };
            match launch {
                Ok(process) => {
                    let registration = scope.register(
                        ResourceKind::Process,
                        format!("Process {}", process.group.0),
                    );
                    scope
                        .tasks
                        .spawn(async move {
                            let _registration = registration;
                            process.run(cancel, limits).await
                        })
                        .await
                        .map_err(anyhow::Error::from)
                        .and_then(std::convert::identity)
                }
                Err(error) => Err(error),
            }
        }
    }
}

#[cfg(all(test, any(target_os = "macos", target_os = "linux")))]
mod tests;
