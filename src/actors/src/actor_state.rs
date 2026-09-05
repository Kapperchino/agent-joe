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
    pub cur_context: C,
    pub(crate) turn: crate::turn::TurnState,
    pub(crate) queue: std::collections::VecDeque<crate::turn::FollowUp>,
    pub(crate) role: crate::turn_driver::SessionRole,
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
        dependency: Dependency<C>,
        actor_ref: ActorRef<actor::Message>,
        file_actor: Option<ActorRef<file_actor::Message>>,
    ) -> anyhow::Result<Self> {
        let dep_clone = dependency.clone();
        let history = Self::initial_history(&dependency.context).await;

        let stream_log_path = if dependency.debug_mode {
            let path = PathBuf::from(format!(
                "./logs/stream_{}.jsonl",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs()
            ));
            if let Some(parent) = path.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }
            Some(path)
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
            cur_context: context,
            history,
            llm: dependency.client,
            turn: Default::default(),
            queue: Default::default(),
            role: crate::turn_driver::SessionRole::Interactive,
            reporter: reporter.clone(),
            debug_mode: dependency.debug_mode,
            file_actor,
            stream_processor: StreamProcessor {
                batches: vec![],
                stream_log_path,
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
        let mut history = vec![Message::new(context.get_ctx().await)];
        if let Some(task) = context.initial_task() {
            history.push(Message::new(task.to_owned()));
        }
        history
    }

    pub fn build_request(&self) -> clients::llm::ClientRequest {
        clients::llm::ClientRequest::new(self.history.clone())
            .with_system(self.cur_context.instructions().to_owned())
            .with_tools(self.tool_definitions())
            .with_thinking()
    }

    pub async fn clear_history(&mut self) {
        self.cur_context.clear_task_context();
        self.history = Self::initial_history(&self.cur_context).await;
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
