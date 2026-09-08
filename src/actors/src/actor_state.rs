use crate::actor;
use crate::actor::Dependency;
use crate::background_actors::file_actor;
use crate::event_reporter::EventReporter;
use crate::stream_processor::StreamProcessor;
use analysis::contexts::context::Context;
use clients::llm::{LLmClient, Message};
use common_models::tui_models::State;
use ractor::ActorRef;
use std::path::PathBuf;
use tools::tool_defs::ToolDefinition;

pub struct ActorState<C: Context> {
    pub(crate) request_mode: crate::context::RequestMode,
    pub(crate) context_checkpoint: crate::context::Checkpoint,
    pub(crate) compact_turn: Option<common_models::runtime_ids::TurnId>,
    pub(crate) questions: Vec<crate::session::PendingQuestion>,
    pub(crate) persistence: crate::session_control::Persistence,
    pub cur_context: C,
    pub(crate) turn: crate::turn_machine::TurnMachine,
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

impl<C: Context + Clone + 'static> ActorState<C> {
    pub async fn new(
        dependency: Dependency<C>,
        actor_ref: ActorRef<actor::Message>,
        file_actor: Option<ActorRef<file_actor::Message>>,
    ) -> anyhow::Result<Self> {
        Self::with_mode(dependency, actor_ref, file_actor, ActorMode::Conversation).await
    }

    pub(crate) async fn with_mode(
        mut dependency: Dependency<C>,
        actor_ref: ActorRef<actor::Message>,
        file_actor: Option<ActorRef<file_actor::Message>>,
        mode: ActorMode,
    ) -> anyhow::Result<Self> {
        if matches!(mode, ActorMode::SingleResponse(_)) {
            dependency.tools.clear();
            dependency.runtime.sessions = None;
            dependency.runtime.session = None;
        }
        let history = Self::initial_history(&dependency.context).await;
        if let Some(store) = &dependency.runtime.sessions {
            dependency.tools.push(tools::tool_defs::erased_tool::<
                crate::tools::read_artifact::ReadArtifact,
                C,
                crate::actor::ActorContext<C>,
            >());
            let parent = dependency
                .runtime
                .session
                .as_ref()
                .map(|session| session.id.clone());
            dependency.runtime.session = Some(store.create(
                dependency.client.session_provider(),
                parent,
                history.clone(),
            )?);
        }
        if let Some(session) = &dependency.runtime.session
            && session.snapshot()?.parent.is_none()
        {
            dependency.runtime.scope.changes = session.change_tracker(Default::default());
        }
        let dep_clone = dependency.clone();

        let stream_log = if dependency.debug_mode {
            let path = PathBuf::from(format!(
                "./logs/stream_{}.jsonl",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs()
            ));
            let file = dependency.runtime.scope.workspace()?.open_append(&path)?;
            Some(tokio::fs::File::from_std(file))
        } else {
            None
        };

        let request_mode = match &mode {
            ActorMode::Conversation => crate::context::RequestMode::Continue,
            ActorMode::SingleResponse(_) => crate::context::RequestMode::SingleResponse,
        };
        let reporter = match mode {
            ActorMode::Conversation => EventReporter::Interactive {
                actor_id: dependency.context.get_id(),
                tui_tx: dependency.tui_tx.clone(),
            },
            ActorMode::SingleResponse(reporter) => reporter,
        };

        Ok(Self {
            request_mode,
            context_checkpoint: Default::default(),
            compact_turn: None,
            questions: Vec::new(),
            persistence: crate::session_control::Persistence::Ready,
            cur_context: dependency.context,
            history,
            llm: dependency.client,
            turn: crate::turn_machine::TurnMachine::new(
                dep_clone.runtime.scope.clone(),
                request_mode,
            ),
            reporter: reporter.clone(),
            debug_mode: dependency.debug_mode,
            file_actor,
            stream_processor: StreamProcessor {
                batches: vec![],
                stream_log,
                token_count: Default::default(),
                reporter,
                cur_state: State::Ready,
                debug: dependency.debug_mode,
            },
            dependency: dep_clone,
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

    #[cfg(test)]
    pub fn build_request(&self) -> clients::llm::ClientRequest {
        clients::llm::ClientRequest::new(self.history.clone())
            .with_system(self.cur_context.effective_instructions().unwrap())
            .with_tools(self.tool_definitions())
            .with_thinking()
    }

    pub(crate) fn context_input(
        &self,
        turn: common_models::runtime_ids::TurnId,
        client: &LLmClient,
    ) -> anyhow::Result<crate::context::ContextInput> {
        Ok(crate::context::ContextInput {
            history: self.history.clone(),
            checkpoint: self.context_checkpoint.clone(),
            questions: self.questions.clone(),
            instructions: self.cur_context.effective_instructions()?,
            tools: self.tool_definitions(),
            limits: self
                .dependency
                .runtime
                .context_budget
                .resolve(client.context_window())?,
            native: self.dependency.runtime.native_compaction,
            mode: match self.compact_turn == Some(turn) {
                true => crate::context::RequestMode::Compact,
                false => self.request_mode,
            },
        })
    }

    pub async fn clear_history(&mut self) -> anyhow::Result<()> {
        let mut context = self.cur_context.clone();
        context.clear_task_context();
        let history = Self::initial_history(&context).await;
        if let Some(store) = &self.dependency.runtime.sessions {
            self.dependency.runtime.session =
                Some(store.create(self.llm.session_provider(), None, history.clone())?);
        }
        self.dependency.runtime.scope.changes = self
            .dependency
            .runtime
            .session
            .as_ref()
            .map(|session| session.change_tracker(Default::default()))
            .unwrap_or_default();
        self.cur_context = context;
        self.history = history;
        self.context_checkpoint = Default::default();
        self.compact_turn = None;
        self.questions.clear();
        self.persistence = crate::session_control::Persistence::Ready;
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

    pub(crate) fn executor(
        &self,
        scope: utils::execution::ExecutionScope,
    ) -> crate::scheduler::Executor<C> {
        crate::scheduler::Executor {
            dependency: Dependency {
                runtime: self.dependency.runtime.child(scope),
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
