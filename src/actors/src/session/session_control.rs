use crate::{
    context,
    session::{
        Event, PendingBatch, QueuedInput, ResumableSession, Session, SessionStore,
        session_merge::MergeEvent,
    },
    states::{actor_state::ActorState, turn::FollowUp},
};
use analysis::contexts::context::Context;
use clients::{
    failure::{Failure, FailureKind},
    llm,
};
use commands::command::{Command, ResumeTarget};
use common_models::tui_models::{
    ActorToTuiPacket, SessionMessage, SessionSummary, SessionTranscript, TokenCount,
};
use std::sync::Arc;
use utils::git::worktrees::session::PruneMode;

pub enum Persistence {
    Ready,
    Failed(Failure),
}

impl Persistence {
    fn fail(&mut self, error: anyhow::Error) -> Option<String> {
        match self {
            Self::Ready => {
                let message =
                    format!("Session storage failed: {error}. Automatic continuation stopped.");
                *self = Self::Failed(Failure::new(FailureKind::Tool, message.clone()));
                Some(message)
            }
            Self::Failed(_) => None,
        }
    }
}

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
        turn: &crate::states::turn_machine::TurnMachine,
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

enum SessionReply {
    Message(String),
    Choices(Vec<SessionSummary>),
    Resumed(SessionTranscript),
}

impl<C: Context + Clone + 'static> ActorState<C> {
    pub async fn commit_context(
        &mut self,
        update: context::compactor::ContextUpdate,
    ) -> Result<common_models::tui_models::RequestContext, Failure> {
        if let Some(compaction) = update.compaction {
            let worker = crate::immutable_workers::ImmutableWorker::spawn(
                crate::workers::snapshot_worker::SnapshotWorker,
                compaction.snapshot,
                crate::immutable_workers::ImmutableWorkerDescription {
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
            self.context_checkpoint = self.save_checkpoint(compaction.checkpoint)?;
            let view = self
                .dependency
                .runtime
                .immutable_workers
                .insert(&self.dependency.worker_owner(), worker);
            self.reporter.send(ActorToTuiPacket::ContextNotice(
                format!("Context compacted. Immutable worker {} preserves the older context; use ask_immutable_worker to query it. The saved transcript and full output artifacts remain available.", view.worker_id),
            ));
        }
        if let (Persistence::Ready, Some(message)) = (&self.persistence, update.runtime_update) {
            self.append_history(vec![message]);
        }
        match &self.persistence {
            Persistence::Ready => Ok(update.request),
            Persistence::Failed(failure) => Err(failure.clone()),
        }
    }

    fn save_checkpoint(
        &mut self,
        checkpoint: crate::context::Checkpoint,
    ) -> Result<crate::context::Checkpoint, Failure> {
        self.persist(Event::Compacted {
            context: checkpoint.clone(),
            usage: self.stream_processor.token_count.clone(),
        });
        match &self.persistence {
            Persistence::Ready => Ok(checkpoint),
            Persistence::Failed(failure) => Err(failure.clone()),
        }
    }

    pub fn append_history(&mut self, messages: Vec<llm::Message>) {
        let stored = self
            .dependency
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
        self.history.extend(messages);
        self.history.append(&mut self.deferred_input);
    }

    pub fn persist(&mut self, event: Event) {
        let result = self
            .dependency
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
        if let Some(session) = &self.dependency.runtime.session {
            self.persist(Event::Queued(QueuedInput {
                turn: session.key(input.id),
                prompt: input.prompt.clone(),
            }));
        }
    }

    pub async fn prepare_session_workspace(&mut self) -> anyhow::Result<()> {
        let mut runtime = self.dependency.runtime.clone();
        if let (crate::states::runtime::ExecutionRole::Root, Some(session)) =
            (&runtime.role, &runtime.session)
            && runtime.project.is_some()
            && session.snapshot()?.worktree.is_none()
        {
            let session = session.clone();
            runtime.activate_session(None)?;
            let snapshot = session.snapshot()?;
            match snapshot.worktree {
                Some(_) => {
                    runtime.scope.changes = session.change_tracker(snapshot.changes);
                    self.relocate_session_workspace(runtime).await?;
                }
                None => {}
            }
        }
        Ok(())
    }

    pub async fn relocate_session_workspace(
        &mut self,
        runtime: crate::states::runtime::Runtime,
    ) -> anyhow::Result<()> {
        let mut context = self.cur_context.clone();
        context.clear_task_context();
        context.relocate(runtime.scope.workspace()?.root().to_path_buf())?;
        let fresh = llm::Message::new(context.get_ctx().await);
        self.turn.relocate(runtime.scope.clone())?;
        self.dependency.runtime = runtime;
        self.dependency.context = context.clone();
        self.cur_context = context;
        if let Some(initial) = self.history.first_mut() {
            *initial = fresh;
        }
        self.relocate_watcher()?;
        Ok(())
    }

    pub async fn begin_turn(&mut self, input: FollowUp) {
        self.llm.begin_turn();
        if let Err(error) = self.record_merge(MergeEvent::TaskStarted { turn: input.id }) {
            self.persistence_failed(error);
        }
        self.refresh_interaction();
        if input.prompt.is_some() && self.merge_approval.resolution(input.id).is_none() {
            self.reconcile_plan();
        }
        let scope = self.dependency.runtime.scope.clone();
        let changes = scope.changes.clone();
        let baseline = match (scope.workspace().is_ok(), self.request_mode) {
            (true, mode) if mode != crate::context::RequestMode::SingleResponse => {
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
        if let Some(session) = &self.dependency.runtime.session {
            self.persist(Event::Began(QueuedInput {
                turn: session.key(input.id),
                prompt: input.prompt.clone(),
            }));
        }
        self.history.extend(input.prompt.map(llm::Message::new));
    }

    pub fn prepare_batch(&mut self) {
        if let Some(session) = &self.dependency.runtime.session
            && let Some(batch) = self.turn.batch()
        {
            self.persist(Event::Prepared(PendingBatch::new(session, batch)));
        }
    }

    pub fn persist_report(&mut self, packet: &ActorToTuiPacket) {
        if let Some(session) = &self.dependency.runtime.session
            && let ActorToTuiPacket::TurnChanged {
                turn_id,
                state,
                detail,
            } = packet
        {
            self.persist(Event::Status {
                turn: session.key(turn_id),
                state: *state,
                detail: detail.clone(),
            });
        }
    }

    pub async fn session_command(&mut self, command: Command) {
        let result = self.run_session_command(&command).await;
        let packet = match (command, result) {
            (command, Ok(SessionReply::Message(message))) => {
                ActorToTuiPacket::CommandResult(command, message)
            }
            (_, Ok(SessionReply::Choices(choices))) => {
                ActorToTuiPacket::SessionChoices(Ok(choices))
            }
            (_, Ok(SessionReply::Resumed(transcript))) => {
                ActorToTuiPacket::SessionResumed(Ok(transcript))
            }
            (Command::Resume(ResumeTarget::Picker), Err(error)) => {
                ActorToTuiPacket::SessionChoices(Err(error.to_string()))
            }
            (Command::Resume(ResumeTarget::Session { .. }), Err(error)) => {
                ActorToTuiPacket::SessionResumed(Err(error.to_string()))
            }
            (command, Err(error)) => ActorToTuiPacket::CommandResult(command, error.to_string()),
        };
        self.reporter.send(packet);
    }

    async fn run_session_command(&mut self, command: &Command) -> anyhow::Result<SessionReply> {
        let store = self
            .dependency
            .runtime
            .sessions
            .clone()
            .ok_or_else(|| anyhow::anyhow!("Session storage is not configured"))?;
        let current = self
            .dependency
            .runtime
            .session
            .as_ref()
            .map(|session| session.id.as_str());
        match SessionAction::new(command, &self.turn, current)? {
            SessionAction::List => Self::list_sessions(&store, current).map(SessionReply::Message),
            SessionAction::Prune(mode) => {
                let runtime = &self.dependency.runtime;
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
                Ok(SessionReply::Message(report))
            }
            SessionAction::Pick => store
                .resume_choices(&self.llm.session_provider(), current)
                .map(SessionReply::Choices),
            SessionAction::Current { id } => Ok(SessionReply::Resumed(self.session_transcript(id))),
            SessionAction::Resume { id } => {
                let workspace = self
                    .dependency
                    .runtime
                    .project
                    .clone()
                    .map(Ok)
                    .unwrap_or_else(|| self.dependency.runtime.scope.workspace())?;
                let session =
                    ResumableSession::new(&store, id, &workspace, &self.llm.session_provider())?
                        .resume()?;
                self.restore_session(session, None).await?;
                Ok(SessionReply::Resumed(self.session_transcript(id)))
            }
            SessionAction::New => {
                self.dispatch(crate::states::turn_machine::SessionEvent::Interrupt(
                    crate::states::turn::HistoryDisposition::Retain,
                ))
                .await;
                self.clear_history().await?;
                self.reporter.send(ActorToTuiPacket::SessionChanged);
                self.reporter
                    .send(ActorToTuiPacket::TokensUpdated(TokenCount::default()));
                Ok(SessionReply::Message(
                    "Started a new session. Previous sessions remain available through /sessions."
                        .into(),
                ))
            }
            SessionAction::Fork => {
                let source = self
                    .dependency
                    .runtime
                    .session
                    .as_ref()
                    .map(|session| session.snapshot())
                    .transpose()?
                    .and_then(|snapshot| snapshot.worktree);
                let session = self
                    .dependency
                    .runtime
                    .session
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("There is no current session"))?
                    .fork()?;
                let id = session.id.clone();
                self.restore_session(session, source.as_ref()).await?;
                let workspace = match source {
                    Some(_) => "The conversation uses a separate Git worktree.",
                    None => {
                        "Both conversations use the same workspace; filesystem changes are shared."
                    }
                };
                Ok(SessionReply::Message(format!(
                    "Forked conversation into session {id}. {workspace}"
                )))
            }
        }
    }

    fn session_transcript(&self, id: &str) -> SessionTranscript {
        let messages = self
            .history
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
        source: Option<&utils::git::worktrees::session::SessionWorktree>,
    ) -> anyhow::Result<()> {
        let mut runtime = self.dependency.runtime.clone();
        runtime.session = Some(session.clone());
        runtime.activate_session(source)?;
        let snapshot = session.snapshot()?;
        runtime.scope.changes = session.change_tracker(snapshot.changes);
        let mut context = self.cur_context.clone();
        context.clear_task_context();
        Self::relocate_context(&mut context, &runtime)?;
        self.dependency
            .runtime
            .immutable_workers
            .clear(&self.dependency.worker_owner())
            .await;
        self.dependency.runtime = runtime;
        self.dependency.context = context.clone();
        let fresh = llm::Message::new(context.get_ctx().await);
        self.prompt_cache_key = session.id.clone();
        self.history = std::iter::once(fresh)
            .chain(snapshot.history.into_iter().skip(1))
            .collect();
        self.cur_context = context;
        self.relocate_watcher()?;
        self.merge_approval = snapshot.merge_approval;
        self.stream_processor.clear();
        self.stream_processor.token_count = snapshot.usage;
        self.context_checkpoint = snapshot.context;
        self.questions = snapshot.questions;
        self.planning = snapshot.planning;
        self.deferred_input = snapshot.deferred_input;
        self.turn = crate::states::turn_machine::TurnMachine::new(
            self.dependency.runtime.scope.clone(),
            self.request_mode,
        );
        self.sync_question_gate().await;
        self.compact_turn = None;
        self.dependency
            .runtime
            .workers
            .restore(&session.id, snapshot.workers);
        self.dependency.runtime.session = Some(session);
        self.persistence = Persistence::Ready;
        self.reporter.send(ActorToTuiPacket::TokensUpdated(
            self.stream_processor.token_count.clone(),
        ));
        Ok(())
    }
}
