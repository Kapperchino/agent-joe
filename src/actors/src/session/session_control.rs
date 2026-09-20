use crate::compactor::ContextUpdate;
use crate::immutable_workers::{ImmutableWorker, ImmutableWorkerDescription};
use crate::session::activation::SessionActivation;
use crate::session::persistence::Persistence;
use crate::session::session_merge::MergeEvent;
use crate::session::{Event, PendingBatch, QueuedInput, ResumableSession, Session, SessionStore};
use crate::states::actor_state::ActorState;
use crate::states::runtime::{ExecutionRole, Runtime};
use crate::workers::snapshot_worker::SnapshotWorker;
use analysis::contexts::context::Context;
use clients::response::RequestMode;
use clients::{
    failure::{Failure, FailureKind},
    llm,
};
use commands::command::{Command, ResumeTarget};
use common_models::tui_models::{
    ActorToTuiPacket, RequestContext, SessionMessage, SessionTranscript, TokenCount,
};
use std::sync::Arc;
use turn_engine::machine::{SessionEvent, TurnMachine};
use turn_engine::turn::{FollowUp, HistoryDisposition};
use utils::git::worktrees::session::{PruneMode, SessionWorktree};

enum SessionAction<'a> {
    List,
    Pick,
    Resume { id: &'a str },
    Current { id: &'a str },
    New,
    Fork,
    Prune(PruneMode),
}

impl<'a> SessionAction<'a> {
    fn new(
        command: &'a Command,
        turn: &TurnMachine,
        current: Option<&str>,
    ) -> anyhow::Result<Self> {
        match command {
            Command::Sessions => Ok(Self::List),
            Command::Prune(_) if !turn.is_idle() => Err(anyhow::anyhow!(
                "Interrupt the active turn before pruning worktrees"
            )),
            Command::Resume(_) | Command::New | Command::Fork if !turn.is_idle() => Err(
                anyhow::anyhow!("Interrupt the active turn before switching sessions"),
            ),
            Command::Resume(ResumeTarget::Picker) => Ok(Self::Pick),
            Command::Resume(ResumeTarget::Session { id }) if current == Some(id.as_str()) => {
                Ok(Self::Current { id })
            }
            Command::Resume(ResumeTarget::Session { id }) => Ok(Self::Resume { id }),
            Command::New => Ok(Self::New),
            Command::Fork => Ok(Self::Fork),
            Command::Prune(mode) => Ok(Self::Prune(match mode {
                commands::command::PruneMode::Merged => PruneMode::Merged,
                commands::command::PruneMode::Force => PruneMode::Force,
            })),
            _ => Err(anyhow::anyhow!("Unsupported session command")),
        }
    }
}

impl<C: Context + Clone + 'static> ActorState<C> {
    pub async fn commit_context(
        &mut self,
        update: ContextUpdate,
    ) -> Result<RequestContext, Failure> {
        if let Some(compaction) = update.compaction {
            let worker = ImmutableWorker::spawn(
                SnapshotWorker,
                compaction.snapshot,
                ImmutableWorkerDescription {
                    kind: "snapshot".into(),
                    description: format!(
                        "Older context preserved by compaction generation {}. Includes the compacted exchanges and any earlier compaction memory.",
                        compaction.checkpoint.generation
                    ),
                },
                &self.actor_ref,
            )
            .await
            .map_err(|error| Failure::new(FailureKind::Worker, error.to_string()))?;
            self.persist(Event::Compacted {
                context: compaction.checkpoint.clone(),
                usage: self.stream_processor.token_count.clone(),
            });
            self.conversation
                .commit_checkpoint(self.persistence.committed(compaction.checkpoint)?);
            let view = self
                .runtime
                .immutable_workers
                .insert(&self.runtime.worker_owner(self.context.get_id()), worker);
            self.reporter.send(ActorToTuiPacket::ContextNotice(
                format!("Context compacted. Immutable worker {} preserves the older context; use ask_immutable_worker to query it. The saved transcript and full output artifacts remain available.", view.worker_id),
            ));
        }
        if let (Persistence::Ready, Some(message)) = (&self.persistence, update.runtime_update) {
            self.append_history(vec![message]);
        }
        self.persistence.committed(update.request)
    }

    pub fn append_history(&mut self, messages: Vec<llm::Message>) {
        let stored = self
            .runtime
            .session
            .as_ref()
            .map(|session| session.snapshot())
            .transpose()
            .map(|snapshot| snapshot.and_then(|snapshot| snapshot.pending));
        let messages = match stored {
            Ok(Some(batch)) => batch.messages().into(),
            Ok(None) => messages,
            Err(error) => {
                self.persistence_failed(error);
                messages
            }
        };
        self.persist(Event::History(messages.clone()));
        self.conversation.append(messages);
    }

    pub fn persist(&mut self, event: Event) {
        let result = self
            .runtime
            .session
            .as_ref()
            .map(|session| session.record(event))
            .transpose();
        if let Err(error) = result {
            self.persistence_failed(error);
        }
    }

    pub fn persistence_failed(&mut self, error: anyhow::Error) {
        if let Some(message) = self.persistence.fail(error) {
            self.reporter.send(ActorToTuiPacket::SessionError(message));
        }
    }

    pub fn queue_input(&mut self, input: &FollowUp) {
        if let Some(session) = &self.runtime.session {
            self.persist(Event::Queued(QueuedInput {
                turn: session.key(input.id),
                prompt: input.prompt.clone(),
            }));
        }
    }

    pub async fn prepare_session_workspace(&mut self) -> anyhow::Result<()> {
        let mut runtime = self.runtime.clone();
        match (&runtime.role, &runtime.project, &runtime.session) {
            (ExecutionRole::Root, Some(_), Some(session))
                if session.snapshot()?.worktree.is_none() =>
            {
                let session = session.clone();
                runtime.activate_session(None)?;
                let snapshot = session.snapshot()?;
                if snapshot.worktree.is_some() {
                    runtime.scope.changes = session.change_tracker(snapshot.changes);
                    self.relocate_session_workspace(runtime).await?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    pub async fn relocate_session_workspace(&mut self, runtime: Runtime) -> anyhow::Result<()> {
        let mut context = self.context.clone();
        context.clear_task_context();
        context.relocate(runtime.scope.workspace()?.root().to_path_buf())?;
        let fresh = llm::Message::new(context.get_ctx().await);
        self.turn.relocate(runtime.scope.clone())?;
        self.context = context;
        self.runtime = runtime;
        self.conversation.relocate(fresh);
        self.relocate_watcher()?;
        Ok(())
    }

    pub async fn begin_turn(&mut self, input: FollowUp) {
        self.llm.begin_turn();
        if let Err(error) = self.record_merge(MergeEvent::TaskStarted { turn: input.id }) {
            self.persistence_failed(error);
        }
        self.refresh_interaction();
        if let (Some(_), None) = (&input.prompt, self.merge_approval.resolution(input.id)) {
            self.reconcile_plan();
        }
        let scope = self.runtime.scope.clone();
        let changes = scope.changes.clone();
        let baseline = match (scope.workspace(), self.request_mode) {
            (Ok(_), RequestMode::Continue | RequestMode::Compact) => {
                scope
                    .enter(utils::files::operation(move |workspace| {
                        changes.start(workspace)
                    }))
                    .await
            }
            _ => Ok(()),
        };
        if let Err(error) = baseline {
            self.persistence_failed(error);
        }
        if let Some(session) = &self.runtime.session {
            self.persist(Event::Began(QueuedInput {
                turn: session.key(input.id),
                prompt: input.prompt.clone(),
            }));
        }
        if let Some(prompt) = input.prompt {
            self.conversation.push(llm::Message::new(prompt));
        }
    }

    pub fn prepare_batch(&mut self) {
        if let (Some(session), Some(batch)) = (&self.runtime.session, self.turn.batch()) {
            self.persist(Event::Prepared(PendingBatch::new(session, batch)));
        }
    }

    pub fn persist_report(&mut self, packet: &ActorToTuiPacket) {
        if let (
            Some(session),
            ActorToTuiPacket::TurnChanged {
                turn_id,
                state,
                detail,
            },
        ) = (&self.runtime.session, packet)
        {
            self.persist(Event::Status {
                turn: session.key(turn_id),
                state: *state,
                detail: detail.clone(),
            });
        }
    }

    pub async fn session_command(&mut self, command: Command) {
        let packet =
            self.run_session_command(&command)
                .await
                .unwrap_or_else(|error| match command {
                    Command::Resume(ResumeTarget::Picker) => {
                        ActorToTuiPacket::SessionChoices(Err(error.to_string()))
                    }
                    Command::Resume(ResumeTarget::Session { .. }) => {
                        ActorToTuiPacket::SessionResumed(Err(error.to_string()))
                    }
                    command => ActorToTuiPacket::CommandResult(command, error.to_string()),
                });
        self.reporter.send(packet);
    }

    async fn run_session_command(&mut self, command: &Command) -> anyhow::Result<ActorToTuiPacket> {
        let store = self
            .runtime
            .sessions
            .clone()
            .ok_or_else(|| anyhow::anyhow!("Session storage is not configured"))?;
        let current = self
            .runtime
            .session
            .as_ref()
            .map(|session| session.id.as_str());
        Ok(match SessionAction::new(command, &self.turn, current)? {
            SessionAction::List => ActorToTuiPacket::CommandResult(
                command.clone(),
                Self::list_sessions(&store, current)?,
            ),
            SessionAction::Prune(mode) => {
                let runtime = &self.runtime;
                runtime
                    .interaction
                    .authorize(tools::tool_defs::ToolEffect::Write)?;
                let project = runtime
                    .project
                    .clone()
                    .ok_or_else(|| anyhow::anyhow!("Session project is not configured"))?;
                let report =
                    tokio::task::spawn_blocking(move || store.prune_worktrees(&project, mode))
                        .await??;
                ActorToTuiPacket::CommandResult(command.clone(), report)
            }
            SessionAction::Pick => ActorToTuiPacket::SessionChoices(Ok(
                store.resume_choices(&self.llm.session_provider(), current)?
            )),
            SessionAction::Current { id } => {
                ActorToTuiPacket::SessionResumed(Ok(self.session_transcript(id)))
            }
            SessionAction::Resume { id } => {
                let workspace = self
                    .runtime
                    .project
                    .clone()
                    .map(Ok)
                    .unwrap_or_else(|| self.runtime.scope.workspace())?;
                let session =
                    ResumableSession::new(&store, id, &workspace, &self.llm.session_provider())?
                        .resume()?;
                self.restore_session(session, None).await?;
                ActorToTuiPacket::SessionResumed(Ok(self.session_transcript(id)))
            }
            SessionAction::New => {
                self.dispatch(SessionEvent::Interrupt(HistoryDisposition::Retain))
                    .await;
                self.clear_history().await?;
                self.reporter.send(ActorToTuiPacket::SessionChanged);
                self.reporter
                    .send(ActorToTuiPacket::TokensUpdated(TokenCount::default()));
                ActorToTuiPacket::CommandResult(
                    command.clone(),
                    "Started a new session. Previous sessions remain available through /sessions."
                        .into(),
                )
            }
            SessionAction::Fork => {
                let current = self
                    .runtime
                    .session
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("There is no current session"))?;
                let source = current.snapshot()?.worktree;
                let session = current.fork()?;
                let id = session.id.clone();
                self.restore_session(session, source.as_ref()).await?;
                let workspace = match source {
                    Some(_) => "The conversation uses a separate Git worktree.",
                    None => {
                        "Both conversations use the same workspace; filesystem changes are shared."
                    }
                };
                ActorToTuiPacket::CommandResult(
                    command.clone(),
                    format!("Forked conversation into session {id}. {workspace}"),
                )
            }
        })
    }

    fn session_transcript(&self, id: &str) -> SessionTranscript {
        let messages = self
            .conversation
            .history()
            .iter()
            .skip(1)
            .flat_map(|message| {
                message
                    .content
                    .iter()
                    .filter_map(|block| match (&message.role, block) {
                        (llm::Role::User, llm::ContentBlock::MessageBlock { text, .. }) => {
                            Some(SessionMessage::User(text.clone()))
                        }
                        (llm::Role::Assistant, llm::ContentBlock::MessageBlock { text, .. }) => {
                            Some(SessionMessage::Assistant(text.clone()))
                        }
                        (_, llm::ContentBlock::ToolBlock { name, input, .. }) => {
                            Some(SessionMessage::Tool(format!(
                                "{name}: {}",
                                serde_json::Value::Object(input.clone())
                            )))
                        }
                        (_, llm::ContentBlock::ToolResult { content, .. }) => {
                            Some(SessionMessage::Tool(content.clone()))
                        }
                        (_, llm::ContentBlock::ThinkingBlock { thinking, .. }) => {
                            Some(SessionMessage::Thinking(thinking.clone()))
                        }
                        (_, llm::ContentBlock::OpenAIReasoning(item)) => {
                            Some(SessionMessage::Thinking(
                                item.summary
                                    .iter()
                                    .map(|part| part.text.as_str())
                                    .collect::<Vec<_>>()
                                    .join("\n"),
                            ))
                        }
                        (_, llm::ContentBlock::RuntimeUpdate(_)) => None,
                        (_, llm::ContentBlock::OpenAICompaction(_)) => Some(
                            SessionMessage::Thinking("Provider-compacted context".into()),
                        ),
                    })
            })
            .collect();
        SessionTranscript {
            id: id.to_owned(),
            messages,
        }
    }

    fn list_sessions(store: &SessionStore, current: Option<&str>) -> anyhow::Result<String> {
        let rows = store
            .list()?
            .into_iter()
            .filter(|snapshot| snapshot.parent.is_none())
            .map(|snapshot| {
                let marker = if current == Some(snapshot.id.as_str()) {
                    " (current)"
                } else {
                    ""
                };
                let title: String = snapshot
                    .history
                    .iter()
                    .skip(1)
                    .find_map(|message| match message.role {
                        llm::Role::User => Some(message.text()),
                        llm::Role::Assistant => None,
                    })
                    .unwrap_or_default()
                    .chars()
                    .take(80)
                    .collect();
                format!(
                    "{}  {:?}{marker}  {}",
                    snapshot.id,
                    snapshot.status,
                    title.replace(['\n', '\r'], " ")
                )
            })
            .collect::<Vec<_>>();
        Ok(format!(
            "Sessions in this project:\n{}\nUse /resume to pick a session, /resume <id> to load one directly, or /new to start a fresh conversation.",
            rows.join("\n")
        ))
    }

    async fn restore_session(
        &mut self,
        session: Arc<Session>,
        source: Option<&SessionWorktree>,
    ) -> anyhow::Result<()> {
        let activation =
            SessionActivation::resume(&self.context, &self.runtime, session, source).await?;
        self.activate_session(activation).await?;
        self.sync_question_gate().await;
        self.reporter.send(ActorToTuiPacket::TokensUpdated(
            self.stream_processor.token_count.clone(),
        ));
        Ok(())
    }
}
