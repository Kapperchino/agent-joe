use super::{CommandEntry, Session};
use crate::{
    ProcessLimits,
    isolation::TemporaryDirectory,
    process::{ProcessEnd, ProcessHandle},
    protocol::{CommandEvent, CommandProtection, GuestCommand, Request},
};
use anyhow::Context;
use std::sync::Arc;
use tokio::{process::Command, sync::mpsc};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

pub struct RunningProcess {
    session: Arc<Session>,
    events: mpsc::Receiver<CommandEvent>,
    handle: Arc<ProcessHandle>,
    limits: ProcessLimits,
    temporary: TemporaryDirectory,
}

enum CommandState {
    Running,
    Stopping {
        end: ProcessEnd,
    },
    Complete {
        end: ProcessEnd,
        exit_code: Option<i32>,
    },
}

impl CommandState {
    fn complete(self, end: ProcessEnd, exit_code: Option<i32>) -> Self {
        match self {
            Self::Running => Self::Complete { end, exit_code },
            Self::Stopping { end } => Self::Complete { end, exit_code },
            complete => complete,
        }
    }
}

impl RunningProcess {
    pub(crate) async fn new(
        session: &Arc<Session>,
        command: Command,
        protection: CommandProtection,
        lease: std::fs::File,
        limits: ProcessLimits,
        handle: Arc<ProcessHandle>,
        cancellations: &[CancellationToken],
    ) -> anyhow::Result<Self> {
        session.ready().await?;
        match cancellations.iter().any(CancellationToken::is_cancelled) {
            true => Err(anyhow::anyhow!("Process cancelled before launch")),
            false => {
                let temporary = session.temporary.child()?;
                let id = temporary.id();
                let command = GuestCommand::new(command.as_std())?;
                let (sender, events) = mpsc::channel(32);
                session.commands.lock().unwrap().insert(
                    id,
                    CommandEntry {
                        events: sender,
                        _lease: lease,
                    },
                );
                let process = Self {
                    session: session.clone(),
                    events,
                    handle,
                    limits,
                    temporary,
                };
                let submitted = session
                    .requests
                    .try_send(Request::Run {
                        id,
                        command,
                        protection,
                    })
                    .context("Sandbox session could not accept the command");
                match submitted {
                    Ok(()) => Ok(process),
                    Err(error) => {
                        session.commands.lock().unwrap().remove(&id);
                        Err(error)
                    }
                }
            }
        }
    }

    pub fn id(&self) -> Uuid {
        self.temporary.id()
    }

    pub async fn run(mut self, on_complete: impl FnOnce() + Send) {
        let deadline = tokio::time::sleep(self.limits.timeout);
        tokio::pin!(deadline);
        let mut state = CommandState::Running;
        while !matches!(state, CommandState::Complete { .. }) {
            state = tokio::select! {
                biased;
                _ = self.handle.cancel.cancelled(), if matches!(state, CommandState::Running) => self.stop(ProcessEnd::Cancelled).await,
                _ = &mut deadline, if matches!(state, CommandState::Running) => self.stop(ProcessEnd::TimedOut).await,
                event = self.events.recv() => self.event(event, state).await,
            };
        }
        if let CommandState::Complete { end, exit_code } = state {
            self.handle.complete(end, exit_code);
        }
        self.session.commands.lock().unwrap().remove(&self.id());
        self.temporary.remove();
        on_complete();
        self.handle.done.cancel();
    }

    async fn stop(&self, end: ProcessEnd) -> CommandState {
        match self
            .session
            .requests
            .send(Request::Cancel { id: self.id() })
            .await
        {
            Ok(()) => CommandState::Stopping { end },
            Err(_) => CommandState::Complete {
                end,
                exit_code: None,
            },
        }
    }

    async fn event(&self, event: Option<CommandEvent>, state: CommandState) -> CommandState {
        match event {
            Some(CommandEvent::Output { stream, bytes }) => {
                let accepted =
                    self.handle
                        .append(stream, &bytes, self.limits.output_bytes as usize);
                match state {
                    CommandState::Running if !accepted => self.stop(ProcessEnd::OutputLimit).await,
                    state => state,
                }
            }
            Some(CommandEvent::Exited { exit_code }) => {
                state.complete(ProcessEnd::Exited, exit_code)
            }
            Some(CommandEvent::Failed { error }) => CommandState::Complete {
                end: ProcessEnd::Failed { error },
                exit_code: None,
            },
            None => {
                let end = match self.session.cancel.is_cancelled() {
                    true => ProcessEnd::Cancelled,
                    false => ProcessEnd::Failed {
                        error: "Sandbox session stopped during execution".into(),
                    },
                };
                state.complete(end, None)
            }
        }
    }
}

impl Drop for RunningProcess {
    fn drop(&mut self) {
        let cancellation = self
            .session
            .commands
            .lock()
            .unwrap()
            .contains_key(&self.id())
            .then(|| {
                self.session
                    .requests
                    .try_send(Request::Cancel { id: self.id() })
                    .map_err(|_| ())
            });
        if matches!(cancellation, Some(Err(_))) {
            self.session.cancel.cancel();
        }
    }
}
