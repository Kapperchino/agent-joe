use crate::session::activation::SessionActivation;
use crate::session::conversation::Conversation;
use crate::session::interaction_state::InteractionState;
use crate::session::session_merge::MergeApproval;
use crate::states::runtime::{ExecutionRole, Runtime};
use crate::states::stream_processor::StreamProcessor;
use crate::states::turn_machine::TurnMachine;
use crate::states::{services::ActorServices, workspace::ActiveWorkspace};
use crate::{
    actor::{self, ActorContext, Dependency},
    background_actors::file_actor,
    context::{ContextInput, RequestMode},
    event_reporter::EventReporter,
    session::persistence::Persistence,
};
use analysis::contexts::context::Context;
use clients::llm::LLmClient;
use common_models::{runtime_ids::TurnId, tui_models::State};
use ractor::ActorRef;
use std::sync::Arc;
use tools::tool_defs::{ToolDefinition, erased_tool};
use utils::execution::ExecutionScope;

pub struct ActorState<C: Context> {
    pub conversation: Conversation,
    pub interaction: InteractionState,
    pub request_mode: RequestMode,
    pub persistence: Persistence,
    pub merge_approval: MergeApproval,
    pub turn: TurnMachine,
    pub llm: LLmClient,
    pub file_actor: Option<ActorRef<file_actor::Message>>,
    pub stream_processor: StreamProcessor,
    pub reporter: EventReporter,
    pub actor_ref: ActorRef<actor::Message>,
    pub services: Arc<ActorServices<C>>,
    pub workspace: ActiveWorkspace<C>,
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
                let artifact_tool = match (
                    &dependency.runtime.sessions,
                    dependency.tool("read_artifact"),
                ) {
                    (Some(_), None) if dependency.runtime.role.allows_tool("read_artifact") => {
                        Some(erased_tool::<
                            crate::tools::read_artifact::ReadArtifact,
                            C,
                            ActorContext<C>,
                        >())
                    }
                    _ => None,
                };
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
        let activation = SessionActivation::start(context, runtime, &client).await?;
        let workspace = activation.workspace;
        if let (Some(watcher), Some(project)) =
            (&file_actor, workspace.context().analysis_project())
        {
            watcher.send_message(file_actor::Message::Relocate(project))?;
        }
        let services = Arc::new(ActorServices {
            client: client.clone(),
            tools,
            tui_tx,
            debug_mode,
        });

        let stream_log = services.stream_log(workspace.runtime())?;
        Ok(Self {
            conversation: activation.conversation,
            interaction: activation.interaction,
            request_mode,
            persistence: Persistence::Ready,
            merge_approval: activation.merge_approval,
            llm: client,
            turn: TurnMachine::new(workspace.runtime().scope.clone(), request_mode),
            reporter: reporter.clone(),
            file_actor,
            stream_processor: StreamProcessor {
                batches: Vec::new(),
                stream_log,
                token_count: activation.usage,
                reporter,
                cur_state: State::Ready,
                debug: debug_mode,
            },
            services,
            workspace,
            actor_ref,
        })
    }

    pub fn context_input(&self, turn: TurnId, client: &LLmClient) -> anyhow::Result<ContextInput> {
        let interaction = match self.request_mode {
            RequestMode::SingleResponse => None,
            RequestMode::Continue | RequestMode::Compact => {
                Some(self.workspace.runtime().role.get_guidance())
            }
        };
        let instructions = std::iter::once(self.workspace.context().effective_instructions()?)
            .chain(interaction)
            .collect::<Vec<_>>()
            .join("\n");
        let runtime = match self.request_mode {
            RequestMode::SingleResponse => None,
            _ => Some(clients::runtime_update::RuntimeSnapshot {
                planning: match self.workspace.runtime().role {
                    ExecutionRole::Root => self.interaction.planning().into(),
                    _ => clients::runtime_update::PlanningState {
                        mode: self.workspace.runtime().interaction.mode(),
                        ..Default::default()
                    },
                },
                evidence: match self.workspace.runtime().role {
                    ExecutionRole::Root => self.interaction.planning().evidence.clone(),
                    _ => Default::default(),
                },
                questions: self.interaction.questions().pending().to_vec(),
                workers: self
                    .workspace
                    .runtime()
                    .workers
                    .pending(&self.workspace.worker_owner()),
            }),
        };
        Ok(ContextInput {
            runtime,
            prompt_cache_key: Some(self.conversation.cache_key().to_owned()),
            purpose: match (&self.workspace.runtime().role, self.request_mode) {
                (_, RequestMode::SingleResponse) => clients::llm::RequestPurpose::Compaction,
                (ExecutionRole::Root, _) => clients::llm::RequestPurpose::Conversation,
                _ => clients::llm::RequestPurpose::Worker,
            },
            history: self.conversation.history().to_vec(),
            checkpoint: self.conversation.checkpoint().clone(),
            instructions,
            tools: self.tool_definitions(),
            limits: self
                .workspace
                .runtime()
                .context_budget
                .resolve(client.context_window())?,
            native: self.workspace.runtime().native_compaction,
            mode: self.conversation.request_mode(turn, self.request_mode),
        })
    }

    pub async fn clear_history(&mut self) -> anyhow::Result<()> {
        let activation = SessionActivation::clear(&self.workspace, &self.llm).await?;
        self.activate_session(activation).await
    }

    pub async fn activate_session(
        &mut self,
        activation: SessionActivation<C>,
    ) -> anyhow::Result<()> {
        self.workspace
            .runtime()
            .immutable_workers
            .clear(&self.workspace.worker_owner())
            .await;
        self.turn = TurnMachine::new(
            activation.workspace.runtime().scope.clone(),
            self.request_mode,
        );
        self.workspace = activation.workspace;
        self.conversation = activation.conversation;
        self.interaction = activation.interaction;
        self.merge_approval = activation.merge_approval;
        self.persistence = Persistence::Ready;
        self.stream_processor.clear();
        self.stream_processor.token_count = activation.usage;
        activation.workers.apply(
            &self.workspace.runtime().workers,
            &self.workspace.worker_owner(),
        );
        self.relocate_watcher()?;
        self.restore_merge_question()?;
        self.refresh_interaction();
        Ok(())
    }

    pub fn relocate_watcher(&self) -> anyhow::Result<()> {
        if let (Some(watcher), Some(project)) = (
            &self.file_actor,
            self.workspace.context().analysis_project(),
        ) {
            watcher.send_message(file_actor::Message::Relocate(project))?;
        }
        Ok(())
    }

    pub fn tool_definitions(&self) -> Vec<ToolDefinition> {
        self.services.tool_definitions()
    }

    pub fn executor(&self, scope: ExecutionScope) -> crate::states::scheduler::Executor<C> {
        crate::states::scheduler::Executor {
            workspace: self
                .workspace
                .execution(scope, self.conversation.constraints()),
            services: self.services.clone(),
            actor: self.actor_ref.clone(),
        }
    }

    pub fn change_state(&mut self, new_state: State) {
        self.stream_processor.change_state(new_state)
    }
}
