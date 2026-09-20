use crate::actor::{self, ActorContext, Dependency};
use crate::background_actors::file_actor;
use crate::event_reporter::EventReporter;
use crate::states::actor_mode::ActorMode;
use crate::states::effects;
use crate::states::provider_context::ProviderContext;
use crate::states::provider_session::ProviderSession;
use crate::states::provider_task::ProviderEvent;
use crate::states::runtime::{ExecutionRole, Runtime};
use crate::states::services::ActorServices;
use crate::states::stream_processor::ProviderStream;
use analysis::contexts::context::Context;
use clients::llm::LLmClient;
use clients::response::RequestMode;
use commands::command::Command;
use common_models::runtime_ids::TurnId;
use common_models::tui_models::{ActorToTuiPacket, TokenCount};
use interaction::control::{InteractionAction, InteractionControl};
use merge_workflow::execution::{MergeAction, MergeCompletion};
use ractor::ActorRef;
use session::activation::SessionActivation;
use session::changes::SessionChanges;
use session::control::{SessionCommand, SessionCommands, SessionControl};
use session::persistence::SessionPersistence;
use session::state::SessionState;
use session::transition::SessionRelocation;
use session::turn::SessionTurn;
use std::{collections::VecDeque, sync::Arc};
use turn_engine::machine::{Event, SessionEvent, TurnMachine};
use turn_engine::turn::{HistoryDisposition, Tag};
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

    pub fn provider_context(&self) -> ProviderContext<'_, C> {
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
        self.session_turn().merge().restore_merge_question()?;
        self.interaction_control().refresh_interaction();
        Ok(())
    }

    pub fn relocate_watcher(&self) -> anyhow::Result<()> {
        if let (Some(watcher), Some(project)) = (&self.file_actor, self.context.analysis_project())
        {
            watcher.send_message(file_actor::Message::Relocate(project))?;
        }
        Ok(())
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

    pub async fn sync_question_gate(&mut self) {
        self.interaction_control().refresh_interaction();
        self.dispatch(SessionEvent::QuestionsChanged(
            self.session.interaction.questions().gate(),
        ))
        .await;
    }

    pub(super) async fn offer_merge(&mut self, turn: TurnId) -> anyhow::Result<()> {
        let client = self.llm.clone();
        let mode = self.request_mode;
        if let Some(completion) = self.session_turn().offer_merge(turn, mode, &client).await? {
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
                self.interaction_control().refresh_interaction();
            }
            MergeAction::Resolve { turn } => {
                self.context.refresh_workspace().await?;
                self.interaction_control().refresh_interaction();
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

    pub async fn relocate_session_workspace(&mut self, runtime: Runtime) -> anyhow::Result<()> {
        let relocation = SessionRelocation::new(&self.context, runtime.session_runtime()).await?;
        self.relocate_session(relocation)
    }

    async fn interaction_command(&mut self, command: &Command) -> anyhow::Result<String> {
        match self.session_turn().command(command)? {
            InteractionAction::Reply(message) => Ok(message),
            InteractionAction::Answer(input) => {
                let result = async {
                    match self.session_turn().answer(&input).await? {
                        Some(completion) => self.complete_merge(completion).await,
                        None => Ok(format!("Answered question {}.", input.id)),
                    }
                }
                .await;
                self.sync_question_gate().await;
                result
            }
            InteractionAction::Steer(input) => {
                self.dispatch(SessionEvent::Steer(input)).await;
                Ok("Correction accepted. Active work and queued follow-ups are cancelled; the corrected task continues after cleanup.".into())
            }
        }
    }

    async fn session_command(&mut self, command: &Command) -> anyhow::Result<ActorToTuiPacket> {
        let action = SessionCommands {
            context: &self.context,
            runtime: &self.runtime.session_runtime(),
            client: &self.llm,
            turn: &self.turn,
            conversation: &self.session.conversation,
        }
        .run(command)
        .await?;
        match action {
            SessionCommand::Report(packet) => Ok(packet),
            SessionCommand::Clear => {
                self.dispatch(SessionEvent::Interrupt(HistoryDisposition::Retain))
                    .await;
                self.clear_history().await?;
                self.reporter.send(ActorToTuiPacket::SessionChanged);
                self.reporter
                    .send(ActorToTuiPacket::TokensUpdated(TokenCount::default()));
                Ok(ActorToTuiPacket::CommandResult(
                    command.clone(),
                    "Started a new session. Previous sessions remain available through /sessions."
                        .into(),
                ))
            }
            SessionCommand::Activate { activation, packet } => {
                self.activate_session(*activation).await?;
                self.sync_question_gate().await;
                self.reporter
                    .send(ActorToTuiPacket::TokensUpdated(self.stream.usage()));
                Ok(packet)
            }
        }
    }

    pub async fn dispatch(&mut self, event: impl Into<Event>) {
        let event = event.into();
        let workspace = match self.turn.needs_workspace(&event) {
            true => self.prepare_session_workspace().await,
            false => Ok(()),
        };
        if let Err(error) = workspace {
            self.session_control().persistence.fail(error);
        }
        self.session_turn().observe(&event);
        let mut effects = VecDeque::from(self.turn.transition(event));
        while let Some(effect) = effects.pop_front() {
            let outcome = effects::execute(self, effect).await;
            let next = self.turn.feedback(outcome);
            for effect in next.into_iter().rev() {
                effects.push_front(effect);
            }
        }
    }

    pub async fn provider_event(&mut self, tag: Tag, event: ProviderEvent) {
        if let Some(response) = self.turn.provider_response(tag) {
            let action = self.stream.event(response, tag, event).await;
            let action = self.provider_context().review(action);
            let update = ProviderSession {
                session: &mut self.session,
                runtime: &self.runtime,
                reporter: &self.reporter,
                actor: &self.actor_ref,
                actor_id: self.context.get_id(),
                usage: self.stream.usage(),
            }
            .apply(tag, action)
            .await;
            self.dispatch(SessionEvent::Provider { tag, update }).await;
        }
    }

    #[cfg(test)]
    pub fn visible_history(&self) -> Vec<clients::llm::Message> {
        let mut history = self.session.conversation.history().to_vec();
        if let Some(batch) = self.turn.batch() {
            history.extend(batch.messages());
        }
        history
    }

    pub async fn command(&mut self, command: Command) {
        let result = async {
            Ok(match &command {
                Command::Plan
                | Command::Implement
                | Command::Questions
                | Command::Answer(_)
                | Command::Steer(_) => Some(ActorToTuiPacket::CommandResult(
                    command.clone(),
                    self.interaction_command(&command).await?,
                )),
                Command::Diff | Command::Undo(_) => {
                    let message = SessionChanges {
                        runtime: &self.runtime.session_runtime(),
                    }
                    .run(&command, &self.turn)
                    .await?;
                    Some(ActorToTuiPacket::CommandResult(command.clone(), message))
                }
                Command::Sessions
                | Command::Resume(_)
                | Command::New
                | Command::Fork
                | Command::Prune(_) => Some(self.session_command(&command).await?),
                Command::Compact => {
                    let event = self.session_turn().compact()?;
                    self.dispatch(event).await;
                    None
                }
                Command::Clear => {
                    self.dispatch(SessionEvent::Interrupt(HistoryDisposition::Clear))
                        .await;
                    None
                }
                Command::PrintContext => {
                    self.provider_context().inspect(self.reporter.clone());
                    None
                }
                Command::Logout => {
                    clients::config::Config::delete().await?;
                    Some(ActorToTuiPacket::CommandResult(
                        command.clone(),
                        "Logged out. Removed config".into(),
                    ))
                }
                Command::ChangeModel(name, effort) => {
                    self.llm
                        .change_model_and_effort(name.clone(), effort.clone())
                        .await?;
                    None
                }
            })
        }
        .await;
        self.reporter.command(command, result);
    }

    pub(crate) fn session_control(&mut self) -> SessionControl<'_> {
        self.session
            .control(self.runtime.session_access(&self.reporter))
    }

    pub(crate) fn interaction_control(&mut self) -> InteractionControl<'_, SessionPersistence<'_>> {
        self.session
            .interaction_control(self.runtime.session_access(&self.reporter))
    }

    pub(crate) fn session_turn(&mut self) -> SessionTurn<'_> {
        SessionTurn {
            state: &mut self.session,
            access: self.runtime.session_access(&self.reporter),
            environment: self.runtime.merge_environment(),
            turn: &self.turn,
        }
    }
}
