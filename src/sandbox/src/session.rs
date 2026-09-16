use crate::{
    isolation::{IsolatedCommand, TemporaryDirectory},
    protocol::{CommandEvent, Frame, Request},
    workspace::Workspace,
};
use anyhow::Context;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, BufReader},
    process::ChildStdout,
    sync::{OnceCell, RwLock, mpsc, watch},
};
use tokio_util::{sync::CancellationToken, task::TaskTracker};
use uuid::Uuid;

mod command;
mod transport;

pub use command::RunningProcess;
use transport::Launcher;

pub(crate) struct SessionOwner {
    session: RwLock<OnceCell<Arc<Session>>>,
    cancel: CancellationToken,
}

impl SessionOwner {
    pub(crate) fn new(cancel: CancellationToken) -> Self {
        Self {
            session: RwLock::new(OnceCell::new()),
            cancel: cancel.child_token(),
        }
    }

    pub(crate) async fn get(
        &self,
        workspace: Arc<dyn Workspace>,
        tasks: &TaskTracker,
    ) -> anyhow::Result<Arc<Session>> {
        match self.cancel.is_cancelled() {
            true => Err(anyhow::anyhow!("Sandbox session is closed")),
            false => {
                self.initialize(async {
                    let cancel = self.cancel.clone();
                    let prepared = tasks
                        .spawn_blocking(move || {
                            IsolatedCommand::new(
                                workspace.as_ref(),
                                &|| match cancel.is_cancelled() {
                                    true => Err(anyhow::anyhow!("Sandbox startup cancelled")),
                                    false => Ok(()),
                                },
                            )
                        })
                        .await??;
                    Session::start(prepared, self.cancel.child_token(), tasks)
                })
                .await
            }
        }
    }

    async fn initialize(
        &self,
        startup: impl std::future::Future<Output = anyhow::Result<Arc<Session>>>,
    ) -> anyhow::Result<Arc<Session>> {
        let slot = self.session.read().await;
        let session = slot.get_or_try_init(|| startup).await?;
        session.ready().await
    }

    pub(crate) async fn shutdown(&self) -> anyhow::Result<()> {
        let mut slot = self.session.write().await;
        if let Some(session) = slot.get() {
            session.cancel.cancel();
            session
                .state
                .clone()
                .wait_for(|state| matches!(state, SessionState::Stopped { .. }))
                .await
                .context("Sandbox shutdown stopped before completion")?;
        }
        slot.take();
        Ok(())
    }
}

impl Drop for SessionOwner {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

enum SessionState {
    Starting,
    Ready,
    Stopped { reason: String },
}

pub(crate) struct Session {
    requests: mpsc::Sender<Request>,
    commands: Mutex<HashMap<Uuid, mpsc::Sender<CommandEvent>>>,
    state: watch::Receiver<SessionState>,
    cancel: CancellationToken,
    temporary: TemporaryDirectory,
    pub(crate) rootfs: std::path::PathBuf,
}

impl Session {
    fn start(
        mut prepared: IsolatedCommand,
        cancel: CancellationToken,
        tasks: &TaskTracker,
    ) -> anyhow::Result<Arc<Self>> {
        let launcher = Launcher::new(&mut prepared.command)?;
        let (requests, receiver) = mpsc::channel(64);
        let (state, status) = watch::channel(SessionState::Starting);
        let session = Arc::new(Self {
            requests,
            commands: Mutex::new(HashMap::new()),
            state: status,
            cancel,
            temporary: prepared.temporary,
            rootfs: prepared.runtime.rootfs,
        });
        tasks.spawn(launcher.supervise(session.clone(), receiver, state));
        Ok(session)
    }

    async fn ready(self: &Arc<Self>) -> anyhow::Result<Arc<Self>> {
        let mut state = self.state.clone();
        let current = state
            .wait_for(|state| !matches!(state, SessionState::Starting))
            .await
            .context("Sandbox startup stopped")?;
        match &*current {
            SessionState::Ready => Ok(self.clone()),
            SessionState::Stopped { reason } => {
                Err(anyhow::anyhow!("Sandbox session stopped: {reason}"))
            }
            SessionState::Starting => Err(anyhow::anyhow!("Sandbox is not ready")),
        }
    }

    async fn read_events(
        &self,
        stdout: ChildStdout,
        state: &watch::Sender<SessionState>,
    ) -> anyhow::Result<()> {
        let mut reader = BufReader::new(stdout);
        let mut line = Vec::new();
        while (&mut reader)
            .take(Frame::MAX_BYTES as u64 + 1)
            .read_until(b'\n', &mut line)
            .await?
            > 0
        {
            match Frame::new(&line)? {
                Frame::BootOutput => {}
                Frame::Ready {} => {
                    state.send_replace(SessionState::Ready);
                }
                Frame::Command { id, event } => self.dispatch(id, event).await,
            }
            line.clear();
        }
        Ok(())
    }

    async fn dispatch(&self, id: Uuid, event: CommandEvent) {
        let sender = self.commands.lock().unwrap().get(&id).cloned();
        if let Some(sender) = sender {
            let _ = sender.send(event).await;
        }
    }
}

#[cfg(test)]
#[path = "../tests/unit/session/tests.rs"]
mod tests;
