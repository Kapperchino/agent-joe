use process::{ProcessCommand, ProcessHandle, ProcessOutput};
use std::sync::Arc;
use tokio::process::Command;
use tokio_util::{sync::CancellationToken, task::TaskTracker};
use workspace::Workspace;

pub mod process;
pub mod workspace;

#[cfg(unix)]
mod isolation;
#[cfg(unix)]
mod protocol;
#[cfg(unix)]
mod provision;
#[cfg(unix)]
pub use unix::RunningProcess;

pub struct ProcessLimits {
    timeout: std::time::Duration,
    output_bytes: u64,
}

impl ProcessLimits {
    pub fn new(timeout: std::time::Duration, output_bytes: u64) -> anyhow::Result<Self> {
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

#[derive(Clone)]
pub struct Sandbox {
    workspace: Arc<dyn Workspace>,
    tasks: TaskTracker,
}

impl Sandbox {
    pub fn new(workspace: Arc<dyn Workspace>, tasks: TaskTracker) -> Self {
        Self { workspace, tasks }
    }

    #[cfg(unix)]
    pub async fn launch(
        &self,
        command: Command,
        limits: ProcessLimits,
        handle: Arc<ProcessHandle>,
        mut cancellations: Vec<CancellationToken>,
    ) -> anyhow::Result<RunningProcess> {
        cancellations.push(handle.cancellation());
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        if command.as_std().get_program() == "cargo"
            && command.as_std().get_args().next() != Some(std::ffi::OsStr::new("fmt"))
        {
            isolation::registry::prepare(self.clone(), cancellations.clone()).await?;
        }
        let workspace = self.workspace.clone();
        let preparation_cancel = cancellations.clone();
        let prepared = self
            .tasks
            .spawn_blocking(move || {
                isolation::IsolatedCommand::new(command, workspace.as_ref(), &|| {
                    match preparation_cancel
                        .iter()
                        .any(|cancel| cancel.is_cancelled())
                    {
                        true => Err(anyhow::anyhow!(
                            "Process cancelled before launch during sandbox preparation"
                        )),
                        false => Ok(()),
                    }
                })
            })
            .await??;
        match cancellations.iter().any(|cancel| cancel.is_cancelled()) {
            true => Err(anyhow::anyhow!("Process cancelled before launch")),
            false => unix::RunningProcess::spawn(prepared, handle, limits).await,
        }
    }

    #[cfg(unix)]
    pub async fn capture(
        &self,
        command: Command,
        limits: ProcessLimits,
        mut cancellations: Vec<CancellationToken>,
    ) -> anyhow::Result<ProcessOutput> {
        let handle = ProcessHandle::new(
            ProcessCommand::from_command(&command),
            CancellationToken::new(),
        );
        let _guard = handle.cancellation().drop_guard();
        let process = self
            .launch(command, limits, handle.clone(), cancellations.clone())
            .await?;
        self.tasks.spawn(process.run(|| {}));
        cancellations.push(handle.cancellation());
        tokio::select! {
            _ = handle.wait() => {},
            _ = futures::future::select_all(
                cancellations.iter().map(|cancel| Box::pin(cancel.cancelled()))
            ) => handle.cancellation().cancel(),
        }
        handle.wait().await;
        Ok(handle.output())
    }
}

#[cfg(unix)]
mod unix {
    use super::isolation::{IsolatedCommand, TemporaryDirectory};
    use super::*;
    use crate::process::{OutputStream, ProcessEnd, ProcessHandle};
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

    pub struct RunningProcess {
        child: Child,
        group: ProcessGroup,
        stdout: ChildStdout,
        stderr: ChildStderr,
        temporary: TemporaryDirectory,
        handle: Arc<ProcessHandle>,
        limits: ProcessLimits,
    }

    impl RunningProcess {
        pub(super) async fn spawn(
            mut prepared: IsolatedCommand,
            handle: Arc<ProcessHandle>,
            limits: ProcessLimits,
        ) -> anyhow::Result<Self> {
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
                    handle,
                    limits,
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

        pub fn id(&self) -> u32 {
            self.group.leader
        }

        pub async fn run(self, on_complete: impl FnOnce() + Send) {
            let Self {
                mut child,
                group,
                stdout,
                stderr,
                temporary,
                handle,
                limits,
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
            on_complete();
            handle.done.cancel();
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
}
