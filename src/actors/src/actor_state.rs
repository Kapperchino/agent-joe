use crate::{
    actor::{self, ActorContext, Dependency},
    background_actors::file_actor,
    context::{Checkpoint, ContextInput, RequestMode},
    event_reporter::EventReporter,
    runtime::Runtime,
    session_control::Persistence,
    stream_processor::StreamProcessor,
    turn_machine::TurnMachine,
};
use analysis::contexts::context::Context;
use clients::llm::{LLmClient, Message};
use common_models::{
    interaction::{Planning, Questions},
    runtime_ids::TurnId,
    tui_models::State,
};
use ractor::ActorRef;
use std::path::PathBuf;
use tools::tool_defs::{ToolDefinition, erased_tool};
use utils::execution::ExecutionScope;

pub struct ActorState<C: Context> {
    pub(crate) planning: Planning,
    pub(crate) deferred_input: Vec<Message>,
    pub(crate) request_mode: RequestMode,
    pub(crate) context_checkpoint: Checkpoint,
    pub(crate) compact_turn: Option<TurnId>,
    pub(crate) questions: Questions,
    pub(crate) persistence: Persistence,
    pub cur_context: C,
    pub(crate) turn: TurnMachine,
    pub history: Vec<Message>,
    pub llm: LLmClient,
    pub file_actor: Option<ActorRef<file_actor::Message>>,
    pub stream_processor: StreamProcessor,
    pub reporter: EventReporter,
    pub debug_mode: bool,
    pub actor_ref: ActorRef<actor::Message>,
    pub(crate) dependency: Dependency<C>,
}

pub(crate) enum ActorMode {
    Conversation,
    SingleResponse(EventReporter),
}

impl ActorMode {
    fn configure<C: Context + Clone + 'static>(&self, dependency: Dependency<C>) -> Dependency<C> {
        match self {
            Self::SingleResponse(_) => Dependency {
                tools: Vec::new(),
                runtime: Runtime {
                    sessions: None,
                    session: None,
                    ..dependency.runtime
                },
                ..dependency
            },
            Self::Conversation => {
                let interaction_tools = dependency.runtime.worker.is_none().then(|| {
                    [
                        erased_tool::<
                            crate::tools::request_user_input::RequestUserInput,
                            C,
                            ActorContext<C>,
                        >(),
                        erased_tool::<crate::tools::update_plan::UpdatePlan, C, ActorContext<C>>(),
                    ]
                });
                let artifact_tool = (dependency.runtime.sessions.is_some()
                    && dependency.tool("read_artifact").is_none()
                    && dependency
                        .runtime
                        .worker
                        .as_ref()
                        .is_none_or(|worker| worker.request.allows_tool("read_artifact")))
                .then(erased_tool::<crate::tools::read_artifact::ReadArtifact, C, ActorContext<C>>);
                Dependency {
                    tools: dependency
                        .tools
                        .into_iter()
                        .chain(interaction_tools.into_iter().flatten())
                        .chain(artifact_tool)
                        .collect(),
                    ..dependency
                }
            }
        }
    }

    fn request_mode(&self) -> RequestMode {
        match self {
            Self::Conversation => RequestMode::Continue,
            Self::SingleResponse(_) => RequestMode::SingleResponse,
        }
    }

    fn reporter<C: Context>(self, dependency: &Dependency<C>) -> EventReporter {
        match self {
            Self::Conversation => EventReporter::Interactive {
                actor_id: dependency.context.get_id(),
                tui_tx: dependency.tui_tx.clone(),
            },
            Self::SingleResponse(reporter) => reporter,
        }
    }
}

enum SessionTransition {
    Start,
    Clear,
}

impl SessionTransition {
    fn apply(
        self,
        mut runtime: Runtime,
        client: &LLmClient,
        history: &[Message],
    ) -> anyhow::Result<Runtime> {
        let parent = match self {
            Self::Start => runtime.session.as_ref().map(|session| session.id.clone()),
            Self::Clear => None,
        };
        runtime.session = match &runtime.sessions {
            Some(store) => {
                Some(store.create(client.session_provider(), parent, history.to_vec())?)
            }
            None => runtime.session,
        };
        runtime.scope.changes = match self {
            Self::Start => {
                if let Some(worker) = &runtime.worker {
                    worker.attach_session(runtime.session.clone())?;
                }
                match &runtime.session {
                    Some(session) if session.snapshot()?.parent.is_none() => {
                        session.change_tracker(Default::default())
                    }
                    _ => runtime.scope.changes,
                }
            }
            Self::Clear => runtime
                .session
                .as_ref()
                .map(|session| session.change_tracker(Default::default()))
                .unwrap_or_default(),
        };
        Ok(runtime)
    }
}

impl<C: Context + Clone + 'static> ActorState<C> {
    pub async fn new(
        dependency: Dependency<C>,
        actor_ref: ActorRef<actor::Message>,
        file_actor: Option<ActorRef<file_actor::Message>>,
    ) -> anyhow::Result<Self> {
        Self::with_mode(dependency, actor_ref, file_actor, ActorMode::Conversation).await
    }

    pub(crate) async fn with_mode(
        dependency: Dependency<C>,
        actor_ref: ActorRef<actor::Message>,
        file_actor: Option<ActorRef<file_actor::Message>>,
        mode: ActorMode,
    ) -> anyhow::Result<Self> {
        let dependency = mode.configure(dependency);
        let history = Self::initial_history(&dependency.context).await;
        let dependency = Dependency {
            runtime: SessionTransition::Start.apply(
                dependency.runtime,
                &dependency.client,
                &history,
            )?,
            ..dependency
        };
        let stream_log = Self::stream_log(&dependency)?;
        let request_mode = mode.request_mode();
        let reporter = mode.reporter(&dependency);

        Ok(Self {
            planning: Planning {
                mode: dependency.runtime.interaction.mode(),
                ..Default::default()
            },
            deferred_input: Vec::new(),
            request_mode,
            context_checkpoint: Default::default(),
            compact_turn: None,
            questions: Default::default(),
            persistence: Persistence::Ready,
            cur_context: dependency.context.clone(),
            history,
            llm: dependency.client.clone(),
            turn: TurnMachine::new(dependency.runtime.scope.clone(), request_mode),
            reporter: reporter.clone(),
            debug_mode: dependency.debug_mode,
            file_actor,
            stream_processor: StreamProcessor {
                batches: Vec::new(),
                stream_log,
                token_count: Default::default(),
                reporter,
                cur_state: State::Ready,
                debug: dependency.debug_mode,
            },
            dependency,
            actor_ref,
        })
    }

    fn stream_log(dependency: &Dependency<C>) -> anyhow::Result<Option<tokio::fs::File>> {
        match &dependency.runtime.worker {
            None if dependency.debug_mode => {
                let timestamp = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                let path = PathBuf::from(format!("./logs/stream_{timestamp}.jsonl"));
                let file = dependency.runtime.scope.workspace()?.open_append(&path)?;
                Ok(Some(tokio::fs::File::from_std(file)))
            }
            _ => Ok(None),
        }
    }

    async fn initial_history(context: &C) -> Vec<Message> {
        std::iter::once(Message::new(context.get_ctx().await))
            .chain(
                context
                    .initial_task()
                    .map(|task| Message::new(task.to_owned())),
            )
            .collect()
    }

    #[cfg(test)]
    pub fn build_request(&self) -> clients::llm::ClientRequest {
        clients::llm::ClientRequest::new(self.history.clone())
            .with_system(self.cur_context.effective_instructions().unwrap())
            .with_tools(self.tool_definitions())
            .with_thinking()
    }

    pub(crate) fn context_input(
        &self,
        turn: TurnId,
        client: &LLmClient,
    ) -> anyhow::Result<ContextInput> {
        let pending = self
            .dependency
            .runtime
            .workers
            .pending(&self.dependency.worker_owner());
        let interaction = match self.request_mode {
            RequestMode::SingleResponse => None,
            RequestMode::Continue | RequestMode::Compact => Some(self.interaction_instructions()),
        };
        let instructions = std::iter::once(self.cur_context.effective_instructions()?)
            .chain(interaction)
            .chain(pending)
            .collect::<Vec<_>>()
            .join("\n");
        let planning = match self.request_mode {
            RequestMode::SingleResponse => None,
            _ if self.dependency.runtime.worker.is_some() => None,
            _ if self.planning.plan.steps.is_empty() && self.planning.evidence.is_empty() => None,
            _ => Some(self.planning.clone()),
        };
        Ok(ContextInput {
            planning,
            history: self.history.clone(),
            checkpoint: self.context_checkpoint.clone(),
            questions: self.questions.pending().to_vec(),
            instructions,
            tools: self.tool_definitions(),
            limits: self
                .dependency
                .runtime
                .context_budget
                .resolve(client.context_window())?,
            native: self.dependency.runtime.native_compaction,
            mode: match self.compact_turn {
                Some(compact_turn) if compact_turn == turn => RequestMode::Compact,
                _ => self.request_mode,
            },
        })
    }

    pub async fn clear_history(&mut self) -> anyhow::Result<()> {
        let mut context = self.cur_context.clone();
        context.clear_task_context();
        let history = Self::initial_history(&context).await;
        self.dependency.runtime =
            SessionTransition::Clear.apply(self.dependency.runtime.clone(), &self.llm, &history)?;
        self.cur_context = context;
        self.history = history;
        self.context_checkpoint = Default::default();
        self.compact_turn = None;
        self.questions = Default::default();
        self.planning = Default::default();
        self.deferred_input.clear();
        self.turn = TurnMachine::new(self.dependency.runtime.scope.clone(), self.request_mode);
        self.refresh_interaction();
        self.persistence = Persistence::Ready;
        self.stream_processor.token_count = Default::default();
        Ok(())
    }

    pub fn tool_definitions(&self) -> Vec<ToolDefinition> {
        self.dependency
            .tools
            .iter()
            .map(|tool| tool.definition())
            .collect()
    }

    pub(crate) fn executor(&self, scope: ExecutionScope) -> crate::scheduler::Executor<C> {
        let runtime = self.dependency.runtime.child(scope.clone());
        let runtime = Runtime {
            turn_scope: Some(scope),
            inherited_constraints: runtime
                .inherited_constraints
                .into_iter()
                .chain(
                    self.history
                        .iter()
                        .skip(1)
                        .filter(|message| matches!(message.role, clients::llm::Role::User))
                        .map(clients::llm::Message::text)
                        .filter(|text| !text.is_empty()),
                )
                .collect(),
            ..runtime
        };
        crate::scheduler::Executor {
            dependency: Dependency {
                runtime,
                ..self.dependency.clone()
            },
            context: self.cur_context.clone(),
            actor: self.actor_ref.clone(),
        }
    }

    pub fn change_state(&mut self, new_state: State) {
        self.stream_processor.change_state(new_state)
    }
}
