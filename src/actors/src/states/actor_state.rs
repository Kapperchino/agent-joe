use crate::session::session_merge::MergeApproval;
use crate::session::session_transition::SessionTransition;
use crate::states::runtime::{ExecutionRole, Runtime};
use crate::states::stream_processor::StreamProcessor;
use crate::states::turn_machine::TurnMachine;
use crate::{
    actor::{self, ActorContext, Dependency},
    background_actors::file_actor,
    context::{Checkpoint, ContextInput, RequestMode},
    event_reporter::EventReporter,
    session::session_control::Persistence,
};
use analysis::contexts::context::Context;
use clients::llm::{LLmClient, Message};
use common_models::{
    interaction::{Planning, Questions},
    runtime_ids::TurnId,
    tui_models::State,
};
use ractor::ActorRef;
use tools::tool_defs::{ToolDefinition, erased_tool};
use utils::execution::ExecutionScope;

pub struct ActorState<C: Context> {
    pub prompt_cache_key: String,
    pub planning: Planning,
    pub deferred_input: Vec<Message>,
    pub request_mode: RequestMode,
    pub context_checkpoint: Checkpoint,
    pub compact_turn: Option<TurnId>,
    pub questions: Questions,
    pub persistence: Persistence,
    pub merge_approval: MergeApproval,
    pub cur_context: C,
    pub turn: TurnMachine,
    pub history: Vec<Message>,
    pub llm: LLmClient,
    pub file_actor: Option<ActorRef<file_actor::Message>>,
    pub stream_processor: StreamProcessor,
    pub reporter: EventReporter,
    pub debug_mode: bool,
    pub actor_ref: ActorRef<actor::Message>,
    pub dependency: Dependency<C>,
}

pub enum ActorMode {
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
                let interaction_tools = matches!(dependency.runtime.role, ExecutionRole::Root).then(|| {
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
                    && dependency.runtime.role.allows_tool("read_artifact"))
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
        let history = Self::initial_history(&dependency.context).await;
        let mut dependency = Dependency {
            runtime: SessionTransition::Start.apply(
                dependency.runtime,
                &dependency.client,
                &history,
            )?,
            ..dependency
        };
        Self::relocate_context(&mut dependency.context, &dependency.runtime)?;
        let history = Self::initial_history(&dependency.context).await;
        if let (Some(watcher), Some(project)) = (&file_actor, dependency.context.analysis_project())
        {
            watcher.send_message(file_actor::Message::Relocate(project))?;
        }
        let stream_log = dependency.stream_log()?;
        let request_mode = mode.request_mode();
        let reporter = mode.reporter(&dependency);

        Ok(Self {
            prompt_cache_key: dependency
                .runtime
                .session
                .as_ref()
                .map(|session| session.id.clone())
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
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
            merge_approval: Default::default(),
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

    async fn initial_history(context: &C) -> Vec<Message> {
        std::iter::once(Message::new(context.get_ctx().await))
            .chain(
                context
                    .initial_task()
                    .map(|task| Message::new(task.to_owned())),
            )
            .collect()
    }

    pub fn context_input(&self, turn: TurnId, client: &LLmClient) -> anyhow::Result<ContextInput> {
        let interaction = match self.request_mode {
            RequestMode::SingleResponse => None,
            RequestMode::Continue | RequestMode::Compact => {
                Some(self.dependency.runtime.role.get_guidance())
            }
        };
        let instructions = std::iter::once(self.cur_context.effective_instructions()?)
            .chain(interaction)
            .collect::<Vec<_>>()
            .join("\n");
        let runtime = match self.request_mode {
            RequestMode::SingleResponse => None,
            _ => Some(clients::runtime_update::RuntimeSnapshot {
                planning: match self.dependency.runtime.role {
                    ExecutionRole::Root => (&self.planning).into(),
                    _ => clients::runtime_update::PlanningState {
                        mode: self.dependency.runtime.interaction.mode(),
                        ..Default::default()
                    },
                },
                evidence: match self.dependency.runtime.role {
                    ExecutionRole::Root => self.planning.evidence.clone(),
                    _ => Default::default(),
                },
                questions: self.questions.pending().to_vec(),
                workers: self
                    .dependency
                    .runtime
                    .workers
                    .pending(&self.dependency.worker_owner()),
            }),
        };
        Ok(ContextInput {
            runtime,
            prompt_cache_key: Some(self.prompt_cache_key.clone()),
            purpose: match (&self.dependency.runtime.role, self.request_mode) {
                (_, RequestMode::SingleResponse) => clients::llm::RequestPurpose::Compaction,
                (ExecutionRole::Root, _) => clients::llm::RequestPurpose::Conversation,
                _ => clients::llm::RequestPurpose::Worker,
            },
            history: self.history.clone(),
            checkpoint: self.context_checkpoint.clone(),
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
        let runtime =
            SessionTransition::Clear.apply(self.dependency.runtime.clone(), &self.llm, &history)?;
        Self::relocate_context(&mut context, &runtime)?;
        let history = Self::initial_history(&context).await;
        self.dependency
            .runtime
            .immutable_workers
            .clear(&self.dependency.worker_owner())
            .await;
        self.prompt_cache_key = runtime
            .session
            .as_ref()
            .map(|session| session.id.clone())
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        self.dependency.runtime = runtime;
        self.dependency.context = context.clone();
        self.cur_context = context;
        self.relocate_watcher()?;
        self.merge_approval = Default::default();
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

    pub fn relocate_context(context: &mut C, runtime: &Runtime) -> anyhow::Result<()> {
        if let Some(session) = &runtime.session
            && session.snapshot()?.worktree.is_some()
        {
            context.relocate(runtime.scope.workspace()?.root().to_path_buf())?;
        }
        Ok(())
    }

    pub fn relocate_watcher(&self) -> anyhow::Result<()> {
        if let (Some(watcher), Some(project)) =
            (&self.file_actor, self.cur_context.analysis_project())
        {
            watcher.send_message(file_actor::Message::Relocate(project))?;
        }
        Ok(())
    }

    pub fn tool_definitions(&self) -> Vec<ToolDefinition> {
        self.dependency
            .tools
            .iter()
            .map(|tool| tool.definition())
            .collect()
    }

    pub fn executor(&self, scope: ExecutionScope) -> crate::states::scheduler::Executor<C> {
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
        crate::states::scheduler::Executor {
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
