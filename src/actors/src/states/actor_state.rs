use crate::actor;
use crate::actor::{ActorContext, Dependency};
use crate::background_actors::file_actor;
use crate::event_reporter::EventReporter;
use crate::session::activation::SessionActivation;
use crate::session::persistence::Persistence;
use crate::session::session_merge::MergeApproval;
use crate::states::runtime::{ExecutionRole, Runtime};
use crate::states::services::ActorServices;
use crate::states::stream_processor::StreamOutput;
use analysis::contexts::context::Context;
use clients::llm::LLmClient;
use clients::response::RequestMode;
use common_models::{runtime_ids::TurnId, tui_models::State};
use conversation::Conversation;
use conversation::context::ContextInput;
use interaction::InteractionState;
use ractor::ActorRef;
use response_stream::StreamProcessor;
use std::sync::Arc;
use tools::tool_defs::{ToolDefinition, erased_tool};
use turn_engine::machine::TurnMachine;
use utils::execution::ExecutionScope;

pub struct ActorState<C: Context> {
    pub conversation: Conversation,
    pub interaction: InteractionState,
    pub request_mode: RequestMode,
    pub persistence: Persistence,
    pub merge_approval: MergeApproval,
    pub turn: TurnMachine,
    pub worker_replies: crate::worker::WorkerReplies,
    pub llm: LLmClient,
    pub file_actor: Option<ActorRef<file_actor::Message>>,
    pub stream_processor: StreamProcessor,
    pub reporter: EventReporter,
    pub actor_ref: ActorRef<actor::Message>,
    pub services: Arc<ActorServices<C, ActorContext<C>>>,
    pub context: C,
    pub runtime: Runtime,
    pub stream_output: StreamOutput,
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
        let context = activation.context;
        let runtime = activation.runtime;
        if let (Some(watcher), Some(project)) = (&file_actor, context.analysis_project()) {
            watcher.send_message(file_actor::Message::Relocate(project))?;
        }
        let services = Arc::new(ActorServices {
            client: client.clone(),
            tools,
            tui_tx,
            debug_mode,
        });

        let stream_log = Self::stream_log(&services, &runtime)?;
        Ok(Self {
            conversation: activation.conversation,
            interaction: activation.interaction,
            request_mode,
            persistence: Persistence::Ready,
            merge_approval: activation.merge_approval,
            llm: client,
            turn: TurnMachine::new(runtime.scope.clone(), request_mode),
            worker_replies: Default::default(),
            reporter: reporter.clone(),
            file_actor,
            stream_processor: StreamProcessor {
                batches: Vec::new(),
                token_count: activation.usage,
                cur_state: State::Ready,
            },
            stream_output: StreamOutput {
                stream_log,
                reporter,
            },
            services,
            context,
            runtime,
            actor_ref,
        })
    }

    pub fn context_input(&self, turn: TurnId, client: &LLmClient) -> anyhow::Result<ContextInput> {
        let interaction = match self.request_mode {
            RequestMode::SingleResponse => None,
            RequestMode::Continue | RequestMode::Compact => Some(self.runtime.role.get_guidance()),
        };
        let instructions = std::iter::once(self.context.effective_instructions()?)
            .chain(interaction)
            .collect::<Vec<_>>()
            .join("\n");
        let runtime = match self.request_mode {
            RequestMode::SingleResponse => None,
            _ => Some(clients::runtime_update::RuntimeSnapshot {
                planning: match self.runtime.role {
                    ExecutionRole::Root => self.interaction.planning().into(),
                    _ => clients::runtime_update::PlanningState {
                        mode: self.runtime.interaction.mode(),
                        ..Default::default()
                    },
                },
                evidence: match self.runtime.role {
                    ExecutionRole::Root => self.interaction.planning().evidence.clone(),
                    _ => Default::default(),
                },
                questions: self.interaction.questions().pending().to_vec(),
                workers: self
                    .runtime
                    .workers
                    .pending(&self.runtime.worker_owner(self.context.get_id())),
            }),
        };
        Ok(ContextInput {
            runtime,
            prompt_cache_key: Some(self.conversation.cache_key().to_owned()),
            purpose: match (&self.runtime.role, self.request_mode) {
                (_, RequestMode::SingleResponse) => clients::llm::RequestPurpose::Compaction,
                (ExecutionRole::Root, _) => clients::llm::RequestPurpose::Conversation,
                _ => clients::llm::RequestPurpose::Worker,
            },
            history: self.conversation.history().to_vec(),
            checkpoint: self.conversation.checkpoint().clone(),
            instructions,
            tools: self.tool_definitions(),
            limits: self
                .runtime
                .context_budget
                .resolve(client.context_window())?,
            native: self.runtime.native_compaction,
            mode: self.conversation.request_mode(turn, self.request_mode),
        })
    }

    pub async fn clear_history(&mut self) -> anyhow::Result<()> {
        let activation = SessionActivation::clear(&self.context, &self.runtime, &self.llm).await?;
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
        self.runtime = activation.runtime;
        self.conversation = activation.conversation;
        self.interaction = activation.interaction;
        self.merge_approval = activation.merge_approval;
        self.persistence = Persistence::Ready;
        self.stream_processor.clear();
        self.stream_processor.token_count = activation.usage;
        activation.workers.apply(
            &self.runtime.workers,
            &self.runtime.worker_owner(self.context.get_id()),
        );
        self.relocate_watcher()?;
        self.restore_merge_question()?;
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

    fn stream_log(
        services: &ActorServices<C, ActorContext<C>>,
        runtime: &Runtime,
    ) -> anyhow::Result<Option<tokio::fs::File>> {
        match runtime.role {
            ExecutionRole::Root if services.debug_mode => {
                let timestamp = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                let path = std::path::PathBuf::from(format!("./logs/stream_{timestamp}.jsonl"));
                let workspace = runtime
                    .project
                    .clone()
                    .map(Ok)
                    .unwrap_or_else(|| runtime.scope.workspace())?;
                let file = workspace.open_append(&path)?;
                Ok(Some(tokio::fs::File::from_std(file)))
            }
            _ => Ok(None),
        }
    }

    pub fn tool_definitions(&self) -> Vec<ToolDefinition> {
        self.services.tool_definitions()
    }

    pub fn executor(&self, scope: ExecutionScope) -> crate::states::scheduler::Executor<C> {
        crate::states::scheduler::Executor {
            context: self.context.clone(),
            runtime: self
                .runtime
                .execution(scope, self.conversation.constraints()),
            services: self.services.clone(),
            actor: self.actor_ref.clone(),
        }
    }

    pub fn change_state(&mut self, new_state: State) {
        self.stream_output
            .send(self.stream_processor.change_state(new_state));
    }
}
