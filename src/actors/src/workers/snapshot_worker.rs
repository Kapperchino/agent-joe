use crate::actor::{ActorContext, Dependency, IntoActorErr, Message};
use crate::states::actor_mode::ActorMode;
use crate::states::actor_state::ActorState;
use crate::states::runtime::{ExecutionRole, Runtime};
use crate::worker::{ContextWorker, run_worker};
use analysis::contexts::{context::Context, rust_context::RustContextLineIndexCreator};
use async_trait::async_trait;
use clients::llm::{ClientRequest, LLmClient};
use conversation::context::{ContextBudget, ContextInput, ContextLimits, NativeCompaction};
use conversation::frozen_context::FrozenContext;
use ractor::{ActorProcessingErr, ActorRef};
use std::{path::PathBuf, time::Duration};
use tools::tool_defs::ErasedToolRef;
use utils::execution::ExecutionScope;

pub struct Snapshot {
    context: FrozenContext,
    client: LLmClient,
    limits: ContextLimits,
    timeout: Duration,
}

impl std::fmt::Debug for Snapshot {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Snapshot")
            .field("limits", &self.limits)
            .field("timeout", &self.timeout)
            .finish_non_exhaustive()
    }
}

impl Snapshot {
    pub fn for_compaction(
        request: ClientRequest,
        client: &LLmClient,
        limits: ContextLimits,
        timeout: Duration,
    ) -> anyhow::Result<Self> {
        Self::from_frozen(request, client, limits.snapshot(), timeout)
    }

    pub fn from_input(
        input: ContextInput,
        client: &LLmClient,
        timeout: Duration,
    ) -> anyhow::Result<Self> {
        Self::from_frozen(
            input.request(&input.checkpoint)?,
            client,
            input.limits.snapshot(),
            timeout,
        )
    }

    pub fn new(
        request: ClientRequest,
        client: &LLmClient,
        limits: ContextLimits,
        timeout: Duration,
    ) -> anyhow::Result<Self> {
        let context_window = Self::model_context_window(client, request.model.as_deref());
        let limits = ContextLimits::new(limits.ceiling().min(context_window), limits.response())?;
        Self::from_frozen(request, client, limits, timeout)
    }

    pub(crate) fn from_frozen(
        request: ClientRequest,
        client: &LLmClient,
        limits: ContextLimits,
        timeout: Duration,
    ) -> anyhow::Result<Self> {
        match timeout.is_zero() {
            true => Err(anyhow::anyhow!(
                "A snapshot requires a nonzero request timeout"
            )),
            false => Ok(()),
        }?;
        let frozen = FrozenContext::new(request, limits)?;
        Ok(Self {
            context: frozen,
            client: client.snapshot(),
            limits,
            timeout,
        })
    }

    fn model_context_window(client: &LLmClient, model: Option<&str>) -> usize {
        match (client, model) {
            (LLmClient::Claude { config, .. } | LLmClient::OpenApi { config, .. }, Some(model)) => {
                let mut config = config.get_config();
                config.set_model(model.to_owned());
                config.context_window()
            }
            (LLmClient::Injected(_), Some(model)) => clients::models::context_window(model),
            (_, None) => client.context_window(),
        }
    }

    pub fn max_question_bytes(&self) -> usize {
        self.context.max_question_bytes()
    }

    pub(crate) fn question_request(&self, question: String) -> anyhow::Result<ClientRequest> {
        self.context.question_request(question)
    }

    pub(crate) async fn answer(
        &self,
        question: String,
        parent: ractor::ActorCell,
    ) -> anyhow::Result<String> {
        let request = self.question_request(question)?;
        let (tui_tx, _) = flume::unbounded();
        let text = run_worker(
            SnapshotWorker,
            Dependency {
                client: self.client.snapshot(),
                tools: Vec::new(),
                tui_tx,
                debug_mode: false,
                context: SnapshotContext { request },
                runtime: Runtime {
                    role: ExecutionRole::Helper,
                    scope: ExecutionScope::current().child(),
                    context_budget: ContextBudget::Fixed(self.limits),
                    native_compaction: NativeCompaction::Disabled,
                    request_timeout: self.timeout,
                    compaction_timeout: self.timeout,
                    ..Runtime::default()
                },
            },
            parent,
        )
        .await?;
        match text.trim().is_empty() {
            true => Err(anyhow::anyhow!("Expected a nonempty snapshot answer")),
            false => Ok(text),
        }
    }
}

pub struct SnapshotWorker;

#[async_trait]
impl ContextWorker for SnapshotWorker {
    type C = SnapshotContext;

    fn init_prompt(added: Option<&str>) -> String {
        format!(
            "{}\n{}",
            conversation::frozen_context::QUESTION_INSTRUCTIONS,
            added.unwrap_or_default()
        )
    }

    async fn startup_hook(
        &self,
        myself: ActorRef<Message>,
        dependency: Dependency<Self::C>,
    ) -> Result<ActorState<Self::C>, ActorProcessingErr> {
        let mode = ActorMode::Snapshot(dependency.context.request.clone());
        ActorState::with_mode(dependency, myself, None, mode)
            .await
            .actor_err()
    }

    fn tools() -> Vec<ErasedToolRef<Self::C, ActorContext<Self::C>>> {
        Vec::new()
    }
}

#[derive(Clone)]
pub struct SnapshotContext {
    request: ClientRequest,
}

#[async_trait]
impl Context for SnapshotContext {
    type LineIndexCreator = RustContextLineIndexCreator;

    async fn get_ctx(&self) -> String {
        String::new()
    }

    fn get_root(&self) -> PathBuf {
        PathBuf::new()
    }

    async fn get_files(&self) -> anyhow::Result<Vec<PathBuf>> {
        Ok(Vec::new())
    }

    async fn line_index_creator(&self) -> anyhow::Result<Box<Self::LineIndexCreator>> {
        Err(anyhow::anyhow!("Snapshots have no workspace tools"))
    }

    fn gen_id(&self) -> u64 {
        self.get_id()
    }

    fn get_id(&self) -> u64 {
        0
    }
}

#[cfg(test)]
#[path = "../../tests/unit/snapshot_actor/tests.rs"]
mod tests;
