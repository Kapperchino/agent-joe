use crate::actor::{self, ActorContext, Dependency, Message};
use crate::background_actors::file_actor;
use crate::compactor::{ContextCommit, ContextUpdate};
use crate::event_reporter::EventReporter;
use crate::states::actor_mode::ActorMode;
use crate::states::provider_context::ProviderContext;
use crate::states::provider_task::{ProviderEvent, ProviderTarget, ProviderTask};
use crate::states::runtime::{ExecutionRole, Runtime};
use crate::states::services::ActorServices;
use crate::states::stream_processor::{ProviderAction, ProviderStream};
use crate::workers::snapshot_worker::Snapshot;
use analysis::contexts::context::Context;
use clients::failure::Failure;
use clients::llm::{self, LLmClient};
use clients::response::RequestMode;
use commands::command::{Command, ResumeTarget};
use common_models::interaction::{Answer, PlanUpdate, Question, QuestionPurpose};
use common_models::runtime_ids::TurnId;
use common_models::tui_models::{ActorToTuiPacket, RequestContext, State, TokenCount};
use conversation::context::ContextInput;
use interaction::access::InteractionReadiness;
use interaction::control::{InteractionAction, InteractionControl};
use merge_workflow::execution::{MergeAction, MergeActivity, MergeCompletion, SessionMerge};
use merge_workflow::{MergeDecision, MergeEvent};
use ractor::ActorRef;
use session::activation::SessionActivation;
use session::changes::SessionChanges;
use session::control::{SessionCommand, SessionCommands, SessionControl};
use session::persistence::{Persistence, SessionPersistence};
use session::state::SessionState;
use session::transition::SessionRelocation;
use std::{collections::VecDeque, sync::Arc};
use tools::tool_defs::ToolDefinition;
use turn_engine::machine::{
    Effect, EffectOutcome, Event, ProviderUpdate, SessionEvent, ShutdownScope, TurnMachine,
    WorkerOutcome,
};
use turn_engine::turn::{FollowUp, HistoryDisposition, Tag};
use utils::execution::ExecutionScope;

pub struct ActorState<C: Context> {
    pub session: SessionState,
    pub request_mode: RequestMode,
    pub turn: TurnMachine,
    pub worker_replies: crate::worker::WorkerReplies,
    pub llm: LLmClient,
    pub file_actor: Option<ActorRef<file_actor::Message>>,
    pub stream: ProviderStream,
    pub reporter: EventReporter,
    pub actor_ref: ActorRef<actor::Message>,
    pub services: Arc<ActorServices<C, ActorContext<C>>>,
    pub context: C,
    pub runtime: Runtime,
}

enum AnswerAction {
    Clarification,
    Merge(MergeDecision),
}

impl<C: Context + Clone + 'static> ActorState<C> {
    pub async fn new(
        dependency: Dependency<C>,
        actor_ref: ActorRef<actor::Message>,
        file_actor: Option<ActorRef<file_actor::Message>>,
    ) -> anyhow::Result<Self> {
        Self::with_mode(dependency, actor_ref, file_actor, ActorMode::Conversation).await
    }

    pub async fn with_mode(
        dependency: Dependency<C>,
        actor_ref: ActorRef<actor::Message>,
        file_actor: Option<ActorRef<file_actor::Message>>,
        mode: ActorMode,
    ) -> anyhow::Result<Self> {
        let dependency = mode.configure(dependency);
        let request_mode = mode.request_mode();
        let reporter = mode.reporter(&dependency);
        let Dependency {
            client,
            tools,
            tui_tx,
            debug_mode,
            context,
            runtime,
        } = dependency;
        let activation =
            SessionActivation::start(context, runtime.session_runtime(), &client).await?;
        let context = activation.context;
        let runtime = runtime.with_session(activation.runtime);
        if let ExecutionRole::Worker { session, .. } = &runtime.role {
            session.attach(runtime.session.clone())?;
        }
        if let (Some(watcher), Some(project)) = (&file_actor, context.analysis_project()) {
            watcher.send_message(file_actor::Message::Relocate(project))?;
        }
        let services = Arc::new(ActorServices {
            client: client.clone(),
            tools,
            tui_tx,
            debug_mode,
        });
        let stream = ProviderStream::new(&services, &runtime, reporter.clone(), activation.usage)?;
        Ok(Self {
            session: activation.state,
            request_mode,
            llm: client,
            turn: TurnMachine::new(runtime.scope.clone(), request_mode),
            worker_replies: Default::default(),
            reporter,
            file_actor,
            stream,
            services,
            context,
            runtime,
            actor_ref,
        })
    }

    pub fn context_input(&self, turn: TurnId, client: &LLmClient) -> anyhow::Result<ContextInput> {
        self.provider_context().input(turn, client)
    }

    fn provider_context(&self) -> ProviderContext<'_, C> {
        ProviderContext {
            context: &self.context,
            session: &self.session,
            runtime: &self.runtime,
            request_mode: self.request_mode,
            services: &self.services,
        }
    }

    pub async fn clear_history(&mut self) -> anyhow::Result<()> {
        let activation =
            SessionActivation::clear(&self.context, &self.runtime.session_runtime(), &self.llm)
                .await?;
        self.activate_session(activation).await
    }

    pub async fn activate_session(
        &mut self,
        activation: SessionActivation<C>,
    ) -> anyhow::Result<()> {
        self.runtime
            .immutable_workers
            .clear(&self.runtime.worker_owner(self.context.get_id()))
            .await;
        self.turn = TurnMachine::new(activation.runtime.scope.clone(), self.request_mode);
        self.worker_replies = Default::default();
        self.context = activation.context;
        self.runtime = self.runtime.clone().with_session(activation.runtime);
        self.session = activation.state;
        self.stream.reset(activation.usage);
        activation.workers.apply(
            &self.runtime.workers,
            &self.runtime.worker_owner(self.context.get_id()),
        );
        self.relocate_watcher()?;
        self.session_merge().restore_merge_question()?;
        self.refresh_interaction();
        Ok(())
    }

    pub fn relocate_watcher(&self) -> anyhow::Result<()> {
        if let (Some(watcher), Some(project)) = (&self.file_actor, self.context.analysis_project())
        {
            watcher.send_message(file_actor::Message::Relocate(project))?;
        }
        Ok(())
    }

    pub fn tool_definitions(&self) -> Vec<ToolDefinition> {
        self.services.tool_definitions()
    }

    pub fn executor(&self, scope: ExecutionScope) -> crate::states::scheduler::Executor<C> {
        crate::states::scheduler::Executor {
            context: self.context.clone(),
            runtime: self
                .runtime
                .execution(scope, self.session.conversation.constraints()),
            services: self.services.clone(),
            actor: self.actor_ref.clone(),
        }
    }

    pub fn change_state(&mut self, new_state: State) {
        self.stream.change_state(new_state);
    }

    pub(crate) fn session_control(&mut self) -> SessionControl<'_> {
        self.session
            .control(self.runtime.session_access(&self.reporter))
    }

    pub(crate) fn interaction_control(&mut self) -> InteractionControl<'_, SessionPersistence<'_>> {
        self.session
            .interaction_control(self.runtime.session_access(&self.reporter))
    }

    pub(crate) fn session_merge(&mut self) -> SessionMerge<'_, SessionPersistence<'_>> {
        self.session.merge(
            self.runtime.session_access(&self.reporter),
            self.runtime.merge_environment(),
            match self.turn.is_idle() {
                true => MergeActivity::Idle,
                false => MergeActivity::Active,
            },
        )
    }

    pub fn persist(&mut self, event: session::Event) {
        self.session_control().persistence.record(event);
    }

    pub fn persistence_failed(&mut self, error: anyhow::Error) {
        self.session_control().persistence.fail(error);
    }

    pub fn refresh_interaction(&mut self) {
        self.interaction_control().refresh_interaction();
    }

    pub async fn sync_question_gate(&mut self) {
        self.refresh_interaction();
        self.dispatch(SessionEvent::QuestionsChanged(
            self.session.interaction.questions().gate(),
        ))
        .await;
    }

    pub fn request_question(
        &mut self,
        question: Question,
        scope: &ExecutionScope,
    ) -> anyhow::Result<String> {
        self.interaction_control().request_question(question, scope)
    }

    pub fn update_plan(
        &mut self,
        update: PlanUpdate,
        scope: &ExecutionScope,
    ) -> anyhow::Result<String> {
        self.interaction_control().update_plan(update, scope)
    }

    pub fn merge_input(&mut self, turn: TurnId) -> Option<FollowUp> {
        self.session_merge().merge_input(turn)
    }

    pub async fn offer_merge(&mut self, turn: TurnId) -> anyhow::Result<()> {
        let mode = self
            .session
            .conversation
            .request_mode(turn, self.request_mode);
        let client = self.llm.clone();
        let cache_key = self.session.conversation.cache_key().to_owned();
        if let Some(completion) = self
            .session_merge()
            .offer_merge(turn, mode, &client, &cache_key)
            .await?
        {
            let message = self.complete_merge(completion).await?;
            self.reporter.send(ActorToTuiPacket::ContextNotice(message));
        }
        Ok(())
    }

    async fn complete_merge(&mut self, completion: MergeCompletion) -> anyhow::Result<String> {
        match completion.action {
            MergeAction::None => {}
            MergeAction::Relocate(workspace) => {
                let mut runtime = self.runtime.clone();
                runtime.scope = runtime.scope.relocated(workspace);
                self.relocate_session_workspace(runtime).await?;
                self.refresh_interaction();
            }
            MergeAction::Resolve { turn } => {
                self.context.refresh_workspace().await?;
                self.refresh_interaction();
                self.actor_ref
                    .send_message(actor::Message::ResolveMerge { turn })?;
            }
        }
        Ok(completion.message)
    }

    pub async fn prepare_session_workspace(&mut self) -> anyhow::Result<()> {
        if let Some(relocation) =
            SessionRelocation::prepare(&self.context, &self.runtime.session_runtime()).await?
        {
            self.relocate_session(relocation)?;
        }
        Ok(())
    }

    fn relocate_session(&mut self, relocation: SessionRelocation<C>) -> anyhow::Result<()> {
        self.turn.relocate(relocation.runtime.scope.clone())?;
        self.context = relocation.context;
        self.runtime = self.runtime.clone().with_session(relocation.runtime);
        self.session.conversation.relocate(relocation.message);
        self.relocate_watcher()
    }

    pub async fn commit_context(
        &mut self,
        update: ContextUpdate,
    ) -> Result<RequestContext, Failure> {
        ContextCommit {
            session: &mut self.session,
            runtime: &self.runtime,
            reporter: &self.reporter,
            actor: &self.actor_ref,
            actor_id: self.context.get_id(),
            usage: self.stream.usage(),
        }
        .commit(update)
        .await
    }

    pub async fn begin_turn(&mut self, input: FollowUp) {
        self.llm.begin_turn();
        if let Err(error) = self
            .session_merge()
            .record_merge(MergeEvent::TaskStarted { turn: input.id })
        {
            self.persistence_failed(error);
        }
        self.refresh_interaction();
        if let (Some(_), None) = (
            &input.prompt,
            self.session.merge_approval.resolution(input.id),
        ) {
            self.interaction_control().reconcile_plan();
        }
        let baseline = SessionChanges {
            runtime: &self.runtime.session_runtime(),
        }
        .begin_turn(self.request_mode)
        .await;
        if let Err(error) = baseline {
            self.persistence_failed(error);
        }
        self.session_control().begin_turn(input);
    }

    pub fn prepare_batch(&mut self) {
        self.session
            .control(self.runtime.session_access(&self.reporter))
            .persistence
            .prepare_batch(self.turn.batch());
    }

    pub async fn relocate_session_workspace(&mut self, runtime: Runtime) -> anyhow::Result<()> {
        let relocation = SessionRelocation::new(&self.context, runtime.session_runtime()).await?;
        self.relocate_session(relocation)
    }

    pub async fn interaction_command(&mut self, command: Command) {
        let readiness = match (self.turn.is_idle(), self.turn.accepts_input()) {
            (true, _) => InteractionReadiness::Idle,
            (false, true) => InteractionReadiness::Active,
            (false, false) => InteractionReadiness::Stopping,
        };
        let action = self.interaction_control().command(&command, readiness);
        let result = match action {
            Ok(InteractionAction::Reply(message)) => Ok(message),
            Ok(InteractionAction::Answer(input)) => {
                let result = self.answer_question(&input.id, input.answer).await;
                self.sync_question_gate().await;
                result
            }
            Ok(InteractionAction::Steer(input)) => {
                self.dispatch(SessionEvent::Steer(input)).await;
                Ok("Correction accepted. Active work and queued follow-ups are cancelled; the corrected task continues after cleanup.".into())
            }
            Err(error) => Err(error),
        };
        self.reporter.send(ActorToTuiPacket::CommandResult(
            command,
            result.unwrap_or_else(|error| format!("{error:#}")),
        ));
    }

    async fn answer_question(&mut self, id: &str, answer: Answer) -> anyhow::Result<String> {
        let answered = self.session.interaction.answered(id, answer.clone())?;
        let action = match answered.answer.purpose {
            QuestionPurpose::Clarification => AnswerAction::Clarification,
            QuestionPurpose::Merge => {
                AnswerAction::Merge(self.session_merge().merge_decision(id, &answer)?)
            }
        };
        let message = llm::Message::new(answered.answer.to_string());
        self.interaction_control()
            .apply_interaction(answered.update)?;
        match self.turn.batch() {
            Some(_) => self.session.conversation.defer(message),
            None => self.session.conversation.push(message),
        }
        match action {
            AnswerAction::Clarification => Ok(format!("Answered question {id}.")),
            AnswerAction::Merge(decision) => {
                let completion = self.session_merge().answer_merge(decision).await?;
                self.complete_merge(completion).await
            }
        }
    }

    pub async fn session_command(&mut self, command: Command) {
        let result = async {
            let action = SessionCommands {
                context: &self.context,
                runtime: &self.runtime.session_runtime(),
                client: &self.llm,
                turn: &self.turn,
                conversation: &self.session.conversation,
            }.run(&command).await?;
            match action {
                SessionCommand::Report(packet) => Ok(packet),
                SessionCommand::Clear => {
                    self.dispatch(SessionEvent::Interrupt(HistoryDisposition::Retain)).await;
                    self.clear_history().await?;
                    self.reporter.send(ActorToTuiPacket::SessionChanged);
                    self.reporter.send(ActorToTuiPacket::TokensUpdated(TokenCount::default()));
                    Ok(ActorToTuiPacket::CommandResult(command.clone(), "Started a new session. Previous sessions remain available through /sessions.".into()))
                }
                SessionCommand::Activate { activation, packet } => {
                    self.activate_session(*activation).await?;
                    self.sync_question_gate().await;
                    self.reporter.send(ActorToTuiPacket::TokensUpdated(self.stream.usage()));
                    Ok(packet)
                }
            }
        }.await;
        let packet = result.unwrap_or_else(|error: anyhow::Error| match command {
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

    pub async fn change_command(&mut self, command: Command) {
        let result = SessionChanges {
            runtime: &self.runtime.session_runtime(),
        }
        .run(&command, &self.turn)
        .await;
        self.reporter.send(ActorToTuiPacket::CommandResult(
            command,
            result.unwrap_or_else(|error| format!("Change operation failed: {error:#}")),
        ));
    }

    pub fn capture_snapshot(&self) -> anyhow::Result<Snapshot> {
        self.provider_context()
            .capture_snapshot(&self.turn, &self.llm)
    }

    pub async fn dispatch(&mut self, event: impl Into<Event>) {
        let event = event.into();
        let workspace = match self.turn.needs_workspace(&event) {
            true => self.prepare_session_workspace().await,
            false => Ok(()),
        };
        if let Err(error) = workspace {
            self.persistence_failed(error);
        }
        if matches!(
            &event,
            Event::StopRequested
                | Event::Shutdown
                | Event::Session(SessionEvent::Interrupt(_) | SessionEvent::Steer(_))
        ) {
            self.session_merge().pause_merge();
        }
        let mut effects = VecDeque::from(self.turn.transition(event));
        while let Some(effect) = effects.pop_front() {
            let outcome = self.execute(effect).await;
            let next = self.turn.feedback(outcome);
            for effect in next.into_iter().rev() {
                effects.push_front(effect);
            }
        }
    }

    async fn execute(&mut self, effect: Effect) -> EffectOutcome {
        match effect {
            Effect::QueueInput(input) => {
                self.session_control().queue_input(&input);
                EffectOutcome::Applied
            }
            Effect::BeginTurn(input) => {
                self.begin_turn(input).await;
                EffectOutcome::Applied
            }
            Effect::AppendHistory(messages) => {
                self.session_control().append_history(messages);
                EffectOutcome::Applied
            }
            Effect::ClearHistory => {
                match self.clear_history().await {
                    Ok(()) => {
                        self.reporter
                            .send(ActorToTuiPacket::TokensUpdated(Default::default()));
                        self.reporter.send(ActorToTuiPacket::CommandResult(
                            Command::Clear,
                            "Started a new session. Previous history remains available through /sessions.".into(),
                        ));
                    }
                    Err(error) => self.persistence_failed(error),
                }
                EffectOutcome::Applied
            }
            Effect::ClearStream => {
                self.stream.clear();
                EffectOutcome::Applied
            }
            Effect::PreserveCompletedContent => {
                self.preserve_completed_content();
                EffectOutcome::Applied
            }
            Effect::ChangeState(state) => {
                self.change_state(state);
                EffectOutcome::Applied
            }
            Effect::Report(mut packet) => {
                self.session_control().persistence.persist_report(&packet);
                match (&mut packet, &self.session.persistence) {
                    (
                        ActorToTuiPacket::TurnChanged { state, detail, .. },
                        Persistence::Failed(failure),
                    ) if state.terminal() => {
                        *state = common_models::tui_models::Lifecycle::Failed;
                        *detail = Some(failure.to_string());
                    }
                    _ => {}
                }
                let completed = match &packet {
                    ActorToTuiPacket::TurnChanged {
                        turn_id,
                        state: common_models::tui_models::Lifecycle::Completed,
                        ..
                    } => Some(*turn_id),
                    _ => None,
                };
                self.reporter.send(packet);
                let merge = match completed {
                    Some(turn) => self.offer_merge(turn).await,
                    None => Ok(()),
                };
                if let Err(error) = merge {
                    self.reporter.send(ActorToTuiPacket::SessionError(format!(
                        "Session merge could not continue: {error:#}"
                    )));
                }
                EffectOutcome::Applied
            }
            Effect::LaunchProvider {
                run,
                owner,
                previous,
            } => {
                if let Some(previous) = &previous {
                    previous.cancel.cancel();
                }
                self.persist(session::Event::Usage(self.stream.usage()));
                match &self.session.persistence {
                    Persistence::Ready => {
                        let client = self.llm.snapshot();
                        let input = self.context_input(run.tag.turn, &client);
                        ProviderTask {
                            budget: match &self.runtime.role {
                                ExecutionRole::Worker { execution, .. } => {
                                    Some(execution.budget.clone())
                                }
                                ExecutionRole::Root | ExecutionRole::Helper => None,
                            },
                            target: ProviderTarget {
                                actor: self.actor_ref.clone(),
                                tag: run.tag,
                            },
                            client,
                            timeout: self.runtime.request_timeout,
                        }
                        .spawn(input, &run, &owner, previous);
                    }
                    Persistence::Failed(failure) => {
                        let _ = self.actor_ref.send_message(Message::Provider {
                            tag: run.tag,
                            event: ProviderEvent::Finished(Err(failure.clone())),
                        });
                    }
                }
                EffectOutcome::Applied
            }
            Effect::LaunchTools { jobs, tag, scope } => {
                self.prepare_batch();
                match &self.session.persistence {
                    Persistence::Ready => self.executor(scope).spawn(jobs, tag),
                    Persistence::Failed(failure) => {
                        let failure = tools::tool_error::ToolFailure::new(
                            tools::tool_error::ToolFailureKind::Persistence,
                            tools::tool_error::ToolEffects::NotStarted,
                            failure.to_string(),
                        );
                        let _ = self.actor_ref.send_message(Message::Tools {
                            tag,
                            event: turn_engine::ToolEvent::Finished(Err(failure)),
                        });
                    }
                }
                EffectOutcome::Applied
            }
            Effect::UpdateContext { tag, result } => {
                self.reporter.validation(&result);
                self.interaction_control().record_plan_evidence(&result);
                match self.services.update_context(&mut self.context, &result) {
                    Ok(()) => EffectOutcome::Applied,
                    Err(failure) => EffectOutcome::ContextFailed { tag, failure },
                }
            }
            Effect::Cleanup { turn, scope } => {
                scope.cancel.cancel();
                let actor = self.actor_ref.clone();
                self.runtime.scope.tasks.spawn(async move {
                    scope.finish().await;
                    let _ = actor.send_message(Message::CleanupFinished { turn });
                });
                EffectOutcome::Applied
            }
            Effect::Shutdown(scope) => {
                match scope {
                    ShutdownScope::Session => {}
                    ShutdownScope::Turn(scope) => scope.finish().await,
                }
                self.runtime.scope.finish().await;
                EffectOutcome::ShutdownFinished
            }
            Effect::ReplyWorker { request, outcome } => {
                let outcome = match &self.session.persistence {
                    Persistence::Ready => outcome,
                    Persistence::Failed(failure) => {
                        WorkerOutcome::Failed(turn_engine::WorkerFailure::Turn(failure.clone()))
                    }
                };
                let result = match outcome {
                    WorkerOutcome::Completed => Ok(self
                        .session
                        .conversation
                        .history()
                        .last()
                        .map(llm::Message::text)
                        .unwrap_or_default()),
                    WorkerOutcome::Failed(failure) => Err(failure),
                };
                self.worker_replies.complete(request, result);
                EffectOutcome::Applied
            }
            Effect::StopActor => {
                self.actor_ref.stop(None);
                EffectOutcome::Applied
            }
        }
    }

    pub async fn provider_event(&mut self, tag: Tag, event: ProviderEvent) {
        if let Some(response) = self.turn.provider_response(tag) {
            let update = match self.stream.event(response, tag, event).await {
                ProviderAction::Update(update) => update,
                ProviderAction::Usage(usage) => {
                    self.persist(session::Event::Usage(usage.clone()));
                    self.reporter.send(ActorToTuiPacket::TokensUpdated(usage));
                    ProviderUpdate::Progress(clients::response::StreamNextStep::Noop)
                }
                ProviderAction::Commit { update, reply } => {
                    let result = self.commit_context(update).await.map(|request| {
                        self.reporter
                            .send(ActorToTuiPacket::ContextUpdated(request));
                    });
                    let _ = reply.send(result);
                    ProviderUpdate::Progress(clients::response::StreamNextStep::Noop)
                }
            };
            let update = self.provider_context().review(update);
            if let ProviderUpdate::Finished(Err(failure)) = &update {
                tracing::warn!(
                    turn_id = %tag.turn,
                    operation_id = %tag.operation,
                    error = %failure,
                    "Provider request failed"
                );
            }
            if matches!(
                update,
                ProviderUpdate::Finished(_) | ProviderUpdate::ReconcilePlan { .. }
            ) {
                self.persist(session::Event::Usage(self.stream.usage()));
            }
            self.dispatch(SessionEvent::Provider { tag, update }).await;
        }
    }

    fn preserve_completed_content(&mut self) {
        if let Some(message) = self.stream.take_completed() {
            self.session_control().append_history(vec![message]);
        }
    }

    #[cfg(test)]
    pub fn visible_history(&self) -> Vec<llm::Message> {
        let mut history = self.session.conversation.history().to_vec();
        if let Some(batch) = self.turn.batch() {
            history.extend(batch.messages());
        }
        history
    }

    pub async fn command(&mut self, command: Command) {
        match command {
            Command::Plan | Command::Implement | Command::Questions | Command::Answer(..) | Command::Steer(_) => self.interaction_command(command).await,
            Command::Diff | Command::Undo(_) => self.change_command(command).await,
            Command::Sessions | Command::Resume(_) | Command::New | Command::Fork | Command::Prune(_) => {
                self.session_command(command).await
            }
            Command::Compact => {
                match self.turn.is_idle() {
                    true => {
                        let follow_up = turn_engine::turn::FollowUp::new(None);
                        self.session.conversation.compact(follow_up.id);
                        self.dispatch(SessionEvent::Start(follow_up)).await;
                    }
                    false => self.reporter.send(ActorToTuiPacket::CommandResult(Command::Compact, "Interrupt the active turn before compacting manually. Automatic compaction runs between complete tool exchanges.".into())),
                }
            }
            Command::Clear => {
                self.dispatch(SessionEvent::Interrupt(HistoryDisposition::Clear))
                    .await
            }
            Command::PrintContext => {
                let context = self.context.clone();
                let reporter = self.reporter.clone();
                let scope = self.runtime.scope.clone();
                scope.tasks.clone().spawn(async move {
                    tokio::select! {
                        _ = scope.cancel.cancelled() => {},
                        text = scope.enter(context.inspect_context()) => reporter.send(ActorToTuiPacket::CommandResult(Command::PrintContext, text.unwrap_or_else(|error| format!("Context inspection failed: {error}")))),
                    }
                });
            }
            Command::Logout => {
                let result = clients::config::Config::delete()
                    .await
                    .map(|_| "Logged out. Removed config".to_owned())
                    .unwrap_or_else(|err| format!("Deletion failed: {err}"));
                self.reporter
                    .send(ActorToTuiPacket::CommandResult(Command::Logout, result));
            }
            Command::ChangeModel(name, effort) => {
                if let Err(error) = self
                    .llm
                    .change_model_and_effort(name.clone(), effort.clone())
                    .await
                {
                    self.reporter.send(ActorToTuiPacket::CommandResult(
                        Command::ChangeModel(name, effort),
                        error.to_string(),
                    ));
                }
            }
        }
    }

    #[cfg(test)]
    pub fn build_request(&self) -> clients::llm::ClientRequest {
        clients::llm::ClientRequest::new(self.session.conversation.history().to_vec())
            .with_system(self.context.effective_instructions().unwrap())
            .with_tools(self.tool_definitions())
            .with_thinking()
    }
}
