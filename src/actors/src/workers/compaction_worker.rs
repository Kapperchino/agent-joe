use crate::{
    actor::{ActorContext, Dependency, IntoActorErr, Message},
    actor_state::{ActorMode, ActorState},
    context::{ContextBudget, ContextLimits, Memory, NativeCompaction},
    event_reporter::EventReporter,
    provider_task::ProviderTask,
    runtime::Runtime,
    worker::{Worker, run_worker},
};
use analysis::contexts::{context::Context, rust_context::RustContextLineIndexCreator};
use async_trait::async_trait;
use clients::llm;
use ractor::{ActorProcessingErr, ActorRef};
use std::path::PathBuf;
use tools::tool_defs::ErasedToolRef;
use utils::execution::ExecutionScope;

pub(crate) struct CompactionWorker {
    reporter: EventReporter,
}

const PROMPT: &str = include_str!("resources/compaction_worker.md");

#[async_trait]
impl Worker for CompactionWorker {
    type C = CompactionContext;

    fn init_prompt(added: Option<&str>) -> String {
        format!("{PROMPT}\n{}", added.unwrap_or_default())
    }

    async fn startup_hook(
        &self,
        myself: ActorRef<Message>,
        dependency: Dependency<Self::C>,
    ) -> Result<ActorState<Self::C>, ActorProcessingErr> {
        ActorState::with_mode(
            dependency,
            myself,
            None,
            ActorMode::SingleResponse(self.reporter.clone()),
        )
        .await
        .actor_err()
    }

    fn tools() -> Vec<ErasedToolRef<Self::C, ActorContext<Self::C>>> {
        vec![]
    }
}

impl CompactionWorker {
    pub(crate) async fn run(
        task: &ProviderTask,
        messages: &[llm::Message],
        limits: ContextLimits,
    ) -> anyhow::Result<Memory> {
        let (tui_tx, _) = flume::unbounded();
        let text = run_worker(
            Self {
                reporter: EventReporter::Compaction(task.target.clone()),
            },
            Dependency {
                client: task.client.clone(),
                tools: Self::tools(),
                tui_tx,
                debug_mode: false,
                context: CompactionContext::new(messages, limits),
                runtime: Runtime {
                    scope: ExecutionScope::current().child(),
                    context_budget: ContextBudget::Fixed(limits),
                    native_compaction: NativeCompaction::Disabled,
                    request_timeout: task.timeout,
                    ..Runtime::default()
                },
            },
            task.target.actor.clone(),
        )
        .await?;
        Memory::summary(text, limits)
    }
}

#[derive(Clone)]
pub(crate) struct CompactionContext {
    history: String,
    instructions: String,
}

impl CompactionContext {
    fn new(messages: &[llm::Message], limits: ContextLimits) -> Self {
        Self {
            history: messages
                .iter()
                .map(|message| format!("{:?}:\n{message}", message.role))
                .collect::<Vec<_>>()
                .join("\n\n"),
            instructions: CompactionWorker::init_prompt(Some(&format!(
                "Keep the handoff under {} UTF-8 bytes.",
                limits.summary_bytes()
            ))),
        }
    }
}

#[async_trait]
impl Context for CompactionContext {
    type LineIndexCreator = RustContextLineIndexCreator;

    async fn get_ctx(&self) -> String {
        self.history.clone()
    }

    fn instructions(&self) -> &str {
        &self.instructions
    }

    fn get_root(&self) -> PathBuf {
        PathBuf::new()
    }

    async fn get_files(&self) -> anyhow::Result<Vec<PathBuf>> {
        Ok(vec![])
    }

    async fn line_index_creator(&self) -> anyhow::Result<Box<Self::LineIndexCreator>> {
        Err(anyhow::anyhow!("Compaction has no workspace tools"))
    }

    fn gen_id(&self) -> u64 {
        self.get_id()
    }

    fn get_id(&self) -> u64 {
        0
    }
}
