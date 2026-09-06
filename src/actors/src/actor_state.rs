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
impl<C: Context + Clone + 'static> ActorState<C> {
    pub async fn new(
        mut dependency: Dependency<C>,
        actor_ref: ActorRef<actor::Message>,
        file_actor: Option<ActorRef<file_actor::Message>>,
    ) -> anyhow::Result<Self> {
        let history = Self::initial_history(&dependency.context).await;
        if let Some(store) = &dependency.runtime.sessions {
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

        let context = dependency.context;
        let actor_id = context.get_id();

        let reporter = EventReporter {
            actor_id,
            tui_tx: dependency.tui_tx.clone(),
        };

        Ok(Self {
            persistence: crate::session_control::Persistence::Ready,
            cur_context: context,
            history,
            llm: dependency.client,
            turn: crate::turn_machine::TurnMachine::new(dep_clone.runtime.scope.clone()),
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

    pub fn build_request(&self) -> clients::llm::ClientRequest {
        clients::llm::ClientRequest::new(self.history.clone())
            .with_system(self.cur_context.instructions().to_owned())
            .with_tools(self.tool_definitions())
            .with_thinking()
    }

    pub async fn clear_history(&mut self) -> anyhow::Result<()> {
        let mut context = self.cur_context.clone();
        context.clear_task_context();
        let history = Self::initial_history(&context).await;
        if let Some(store) = &self.dependency.runtime.sessions {
            self.dependency.runtime.session =
                Some(store.create(self.llm.session_provider(), None, history.clone())?);
        }
        self.cur_context = context;
        self.history = history;
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
