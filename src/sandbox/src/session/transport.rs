use super::{Session, SessionState};
use crate::protocol::Request;
use anyhow::Context;
use std::{process::Stdio, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::{Child, ChildStderr, ChildStdin, ChildStdout, Command},
    sync::{mpsc, watch},
};

pub(super) struct Launcher {
    child: Child,
    group: ProcessGroup,
    stdin: ChildStdin,
    stdout: ChildStdout,
    stderr: ChildStderr,
}

impl Launcher {
    pub(super) fn new(command: &mut Command) -> anyhow::Result<Self> {
        command
            .process_group(0)
            .kill_on_drop(true)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().context("Cannot launch sandbox session")?;
        let group = ProcessGroup {
            leader: child.id().context("Sandbox has no process ID")?,
        };
        let stdin = child.stdin.take().context("Sandbox has no input pipe")?;
        let stdout = child.stdout.take().context("Sandbox has no output pipe")?;
        let stderr = child.stderr.take().context("Sandbox has no error pipe")?;
        Ok(Self {
            child,
            group,
            stdin,
            stdout,
            stderr,
        })
    }

    pub(super) async fn supervise(
        self,
        session: Arc<Session>,
        requests: mpsc::Receiver<Request>,
        state: watch::Sender<SessionState>,
    ) {
        let Self {
            mut child,
            group,
            stdin,
            stdout,
            stderr,
        } = self;
        let error_reader = tokio::spawn(read_errors(stderr));
        let reason = tokio::select! {
            biased;
            _ = session.cancel.cancelled() => "Joe closed the sandbox session".to_owned(),
            reason = async {
                match tokio::time::timeout(Duration::from_secs(30), session.ready()).await {
                    Ok(Ok(_)) => std::future::pending().await,
                    Ok(Err(error)) => format!("Sandbox startup stopped: {error:#}"),
                    Err(_) => "Sandbox did not become ready within 30 seconds".to_owned(),
                }
            } => reason,
            result = child.wait() => format!("Sandbox launcher exited: {result:?}"),
            result = session.read_events(stdout, &state) => format!("Sandbox output ended: {result:#?}"),
            result = write_requests(stdin, requests) => format!("Sandbox input ended: {result:#?}"),
        };
        drop(group);
        let _ = child.kill().await;
        let _ = child.wait().await;
        let errors = error_reader.await.unwrap_or_default();
        let reason = format!("{reason}\n{}", String::from_utf8_lossy(&errors));
        session.commands.lock().unwrap().clear();
        session.temporary.remove();
        state.send_replace(SessionState::Stopped { reason });
    }
}

struct ProcessGroup {
    leader: u32,
}

impl Drop for ProcessGroup {
    fn drop(&mut self) {
        unsafe {
            libc::kill(-(self.leader as i32), libc::SIGKILL);
        }
    }
}

async fn write_requests(
    mut stdin: ChildStdin,
    mut requests: mpsc::Receiver<Request>,
) -> anyhow::Result<()> {
    while let Some(request) = requests.recv().await {
        let mut bytes = serde_json::to_vec(&request)?;
        bytes.push(b'\n');
        stdin.write_all(&bytes).await?;
        stdin.flush().await?;
    }
    Ok(())
}

async fn read_errors(mut stderr: ChildStderr) -> Vec<u8> {
    let mut errors = Vec::new();
    let mut bytes = [0; 8192];
    while let Ok(count @ 1..) = stderr.read(&mut bytes).await {
        let length = count.min(16384_usize.saturating_sub(errors.len()));
        errors.extend_from_slice(&bytes[..length]);
    }
    errors
}
