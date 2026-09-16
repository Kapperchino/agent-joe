use process::{ProcessCommand, ProcessHandle, ProcessOutput};
use std::sync::Arc;
use tokio::process::Command;
use tokio_util::{sync::CancellationToken, task::TaskTracker};
use workspace::Workspace;

pub mod process;
pub mod workspace;

#[cfg(unix)]
mod configuration;
#[cfg(unix)]
mod isolation;
#[cfg(unix)]
mod protocol;
#[cfg(unix)]
mod provision;
#[cfg(unix)]
mod session;
#[cfg(unix)]
pub use session::RunningProcess;

#[cfg(test)]
#[path = "../tests/unit/limits/tests.rs"]
mod limits_tests;

pub struct ProcessLimits {
    timeout: std::time::Duration,
    output_bytes: u64,
}

impl ProcessLimits {
    pub const MAX_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3600);

    pub fn new(timeout: std::time::Duration, output_bytes: u64) -> anyhow::Result<Self> {
        match !timeout.is_zero()
            && timeout <= Self::MAX_TIMEOUT
            && output_bytes > 0
            && output_bytes <= 16 * 1024 * 1024
        {
            true => Ok(Self {
                timeout,
                output_bytes,
            }),
            false => Err(anyhow::anyhow!(
                "Process limits must fit within one hour and 16 MiB per stream"
            )),
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
    #[cfg(unix)]
    session: Arc<session::SessionOwner>,
}

impl Sandbox {
    pub fn new(
        workspace: Arc<dyn Workspace>,
        tasks: TaskTracker,
        cancel: CancellationToken,
    ) -> Self {
        Self {
            workspace,
            tasks,
            #[cfg(unix)]
            session: Arc::new(session::SessionOwner::new(cancel)),
        }
    }

    #[cfg(unix)]
    pub async fn start(&self) -> anyhow::Result<()> {
        self.session
            .get(self.workspace.clone(), &self.tasks)
            .await?;
        Ok(())
    }

    #[cfg(unix)]
    pub async fn shutdown(&self) -> anyhow::Result<()> {
        self.session.shutdown().await
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
        match cancellations.iter().any(|cancel| cancel.is_cancelled()) {
            true => Err(anyhow::anyhow!("Process cancelled before launch")),
            false => Ok(()),
        }?;
        let session = self
            .session
            .get(self.workspace.clone(), &self.tasks)
            .await?;
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        if command.as_std().get_program() == "cargo"
            && command.as_std().get_args().next() != Some(std::ffi::OsStr::new("fmt"))
        {
            isolation::registry::prepare(self.clone(), cancellations.clone()).await?;
        }
        let workspace = self.workspace.clone();
        let protection = self
            .tasks
            .spawn_blocking(move || protocol::CommandProtection::new(workspace.as_ref()))
            .await??;
        RunningProcess::new(
            &session,
            command,
            protection,
            limits,
            handle,
            &cancellations,
        )
        .await
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
            _ = futures::future::select_all(cancellations.iter().map(|cancel| Box::pin(cancel.cancelled()))) => handle.cancellation().cancel(),
        }
        handle.wait().await;
        Ok(handle.output())
    }
}
