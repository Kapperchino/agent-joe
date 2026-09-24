use crate::{
    actor::Message,
    immutable_workers::{
        ImmutableWorker, ImmutableWorkerDescription, ImmutableWorkerRegistry, ImmutableWorkerView,
    },
    workers::knowledge_worker::{KnowledgeWorker, KnowledgeWorkerState, request},
};
use analysis::knowledge::{KnowledgeBudget, KnowledgeIndex, RoutePage, SymbolInspection};
use clients::llm::{LLmClient, SessionProvider};
use common_models::knowledge::{SemanticProfile, SourcePath, SymbolId};
use conversation::context::{ContextBudget, estimated_tokens};
use ractor::ActorRef;
use serde::Serialize;
use std::{
    collections::BTreeSet,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio_util::sync::CancellationToken;
use utils::{
    execution::ExecutionScope,
    knowledge::{Fingerprint, PreparedKnowledge},
    workspace::{Access, WorkspacePolicy},
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ModelIdentity {
    provider: SessionProvider,
    model: String,
    window: usize,
}

impl ModelIdentity {
    fn new(client: &LLmClient) -> Self {
        let model = match client {
            LLmClient::Injected(_) => "injected".into(),
            LLmClient::Claude { config, .. } | LLmClient::OpenApi { config, .. } => {
                config.get_config().get_model()
            }
        };
        Self {
            provider: client.session_provider(),
            model,
            window: client.context_window(),
        }
    }
}

pub fn budget(client: &LLmClient, configured: ContextBudget) -> anyhow::Result<KnowledgeBudget> {
    let (window, response) = match configured {
        ContextBudget::Model { response } => (client.context_window(), response),
        ContextBudget::Fixed(limits) => (limits.ceiling(), limits.response()),
    };
    KnowledgeBudget::new(client.context_window(), window, response)
}

enum Validity {
    Current,
    Stale(String),
}

impl Validity {
    fn invalidate(&mut self, reason: &str) {
        match self {
            Self::Current => *self = Self::Stale(reason.into()),
            Self::Stale(_) => {}
        }
    }

    fn current(&self) -> anyhow::Result<()> {
        match self {
            Self::Current => Ok(()),
            Self::Stale(reason) => Err(anyhow::anyhow!("Knowledge generation is stale: {reason}")),
        }
    }
}

pub(crate) struct Freshness {
    fingerprint: Fingerprint,
    workspace: Arc<WorkspacePolicy>,
    client: LLmClient,
    model: ModelIdentity,
    validity: Mutex<Validity>,
    pub(crate) cancel: CancellationToken,
}

impl Freshness {
    fn new(fingerprint: Fingerprint, workspace: Arc<WorkspacePolicy>, client: &LLmClient) -> Self {
        Self {
            fingerprint,
            workspace,
            client: client.clone(),
            model: ModelIdentity::new(client),
            validity: Mutex::new(Validity::Current),
            cancel: CancellationToken::new(),
        }
    }

    fn invalidate(&self, reason: &str) {
        self.validity
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .invalidate(reason);
        self.cancel.cancel();
    }

    fn current(&self) -> anyhow::Result<()> {
        self.validity
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .current()
    }

    pub(crate) async fn check(self: &Arc<Self>) -> anyhow::Result<()> {
        self.current()?;
        let state = self.clone();
        let current =
            tokio::task::spawn_blocking(move || state.fingerprint.is_current(&state.workspace))
                .await?;
        let result = match (current, self.model == ModelIdentity::new(&self.client)) {
            (Ok(true), true) => self.current(),
            (Ok(true), false) => Err(anyhow::anyhow!(
                "Provider model or context window changed; repartition before asking"
            )),
            (Ok(false), _) => Err(anyhow::anyhow!(
                "Discoverable workspace inputs changed; prepare a new semantic graph"
            )),
            (Err(error), _) => Err(error.context("Cannot verify current knowledge inputs")),
        };
        result.inspect_err(|error| self.invalidate(&error.to_string()))
    }
}

pub struct Generation {
    pub index: KnowledgeIndex,
    freshness: Arc<Freshness>,
    workers: Vec<ImmutableWorkerView>,
}

impl Generation {
    fn scope(&self, workspace: &WorkspacePolicy) -> anyhow::Result<()> {
        match workspace.permits_workspace_access(Access::Read)
            && workspace.workspace_identity()? == self.freshness.workspace.workspace_identity()?
        {
            true => Ok(()),
            false => Err(anyhow::anyhow!(
                "Knowledge is restricted to its whole-workspace conversation"
            )),
        }
    }

    pub async fn check(
        &self,
        workspace: &WorkspacePolicy,
        current_budget: KnowledgeBudget,
    ) -> anyhow::Result<()> {
        self.scope(workspace)?;
        match current_budget == self.index.budget {
            true => self.freshness.check().await,
            false => Err(anyhow::anyhow!(
                "Knowledge request budget changed; repartition before asking"
            )),
        }
    }

    fn identity(&self, expected: Option<&str>, offset: usize) -> anyhow::Result<()> {
        match expected {
            Some(id) if id == self.index.generation => Ok(()),
            None if offset == 0 => Ok(()),
            _ => Err(anyhow::anyhow!(
                "Use the current generation identity; paginated results cannot cross generations"
            )),
        }
    }

    pub fn search(
        &self,
        query: &str,
        generation: Option<&str>,
        offset: usize,
        limit: usize,
    ) -> anyhow::Result<RoutedPage> {
        self.identity(generation, offset)?;
        let page = self.index.search(query, offset, limit)?;
        let shards = page
            .hits
            .iter()
            .flat_map(|hit| hit.primary_shards.iter().chain(&hit.related_shards))
            .copied()
            .collect();
        Ok(RoutedPage {
            page,
            workers: self.routes(shards),
        })
    }

    pub fn inspect(&self, symbol: &SymbolId, generation: &str) -> anyhow::Result<RoutedSymbol> {
        self.identity(Some(generation), 0)?;
        let symbol = self.index.inspect(symbol)?;
        let shards = symbol
            .primary_shards
            .iter()
            .chain(&symbol.related_shards)
            .copied()
            .collect();
        Ok(RoutedSymbol {
            workers: self.routes(shards),
            symbol,
        })
    }

    fn routes(&self, shards: BTreeSet<usize>) -> Vec<ShardWorker> {
        shards
            .into_iter()
            .map(|index| ShardWorker {
                index,
                worker: self.workers[index].clone(),
            })
            .collect()
    }

    fn summary(&self, page: WorkerPage) -> GenerationSummary {
        GenerationSummary {
            generation: self.index.generation.clone(),
            profile: self.index.graph.data().profile.clone(),
            model: self.freshness.model.clone(),
            budget: self.index.budget,
            files: self.index.graph.data().sources.len(),
            symbols: self.index.graph.data().symbols.len(),
            relations: self.index.graph.data().relations.len(),
            diagnostics: self.index.graph.data().diagnostics.len(),
            shards: self.index.shards.len(),
            workers: self
                .index
                .shards
                .iter()
                .skip(page.offset)
                .take(page.limit)
                .map(|shard| ShardView {
                    route: ShardWorker {
                        index: shard.summary.index,
                        worker: self.workers[shard.summary.index].clone(),
                    },
                    estimated_tokens: shard.summary.estimated_tokens,
                    owned_bytes: shard.summary.owned_bytes,
                    paths: shard.summary.paths.iter().take(8).cloned().collect(),
                    path_count: shard.summary.paths.len(),
                    symbols: shard.summary.symbol_count,
                })
                .collect(),
            next_offset: (page.offset.saturating_add(page.limit) < self.index.shards.len())
                .then_some(page.offset.saturating_add(page.limit)),
        }
    }

    async fn status(
        &self,
        workspace: &WorkspacePolicy,
        budget: KnowledgeBudget,
        page: WorkerPage,
    ) -> anyhow::Result<Status> {
        self.scope(workspace)?;
        let summary = self.summary(page);
        Ok(match self.check(workspace, budget).await {
            Ok(()) => Status::Ready { summary },
            Err(error) => Status::Stale {
                summary,
                reason: error.to_string(),
            },
        })
    }
}

struct WorkerPage {
    offset: usize,
    limit: usize,
}

impl WorkerPage {
    fn new(offset: usize, limit: usize) -> anyhow::Result<Self> {
        match (offset, limit) {
            (0..=analysis::knowledge::MAX_SHARDS, 1..=32) => Ok(Self { offset, limit }),
            _ => Err(anyhow::anyhow!(
                "Knowledge status requires a limit of 1–32 and an offset up to 256"
            )),
        }
    }
}

#[derive(Serialize)]
pub struct ShardWorker {
    pub index: usize,
    pub worker: ImmutableWorkerView,
}

#[derive(Serialize)]
pub struct ShardView {
    pub route: ShardWorker,
    pub estimated_tokens: usize,
    pub owned_bytes: usize,
    pub paths: Vec<SourcePath>,
    pub path_count: usize,
    pub symbols: usize,
}

#[derive(Serialize)]
pub struct GenerationSummary {
    pub generation: String,
    pub profile: SemanticProfile,
    pub model: ModelIdentity,
    pub budget: KnowledgeBudget,
    pub files: usize,
    pub symbols: usize,
    pub relations: usize,
    pub diagnostics: usize,
    pub shards: usize,
    pub workers: Vec<ShardView>,
    pub next_offset: Option<usize>,
}

#[derive(Serialize)]
pub struct RoutedPage {
    pub page: RoutePage,
    pub workers: Vec<ShardWorker>,
}

#[derive(Serialize)]
pub struct RoutedSymbol {
    pub symbol: SymbolInspection,
    pub workers: Vec<ShardWorker>,
}

#[derive(Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Status {
    Absent,
    Preparing {
        ticket: String,
    },
    Ready {
        summary: GenerationSummary,
    },
    Stale {
        summary: GenerationSummary,
        reason: String,
    },
}

#[derive(Clone)]
pub(crate) enum Slot {
    Preparing {
        ticket: String,
        cancel: CancellationToken,
        previous: Option<Arc<Generation>>,
    },
    Ready(Arc<Generation>),
}

impl Slot {
    pub(crate) fn invalidate(&self, reason: &str) {
        match self {
            Self::Preparing {
                cancel, previous, ..
            } => {
                cancel.cancel();
                previous
                    .iter()
                    .for_each(|generation| generation.freshness.invalidate(reason));
            }
            Self::Ready(generation) => generation.freshness.invalidate(reason),
        }
    }

    fn rollback(self, expected: &str) -> Option<Self> {
        match self {
            Self::Preparing {
                ticket, previous, ..
            } if ticket == expected => previous.map(Self::Ready),
            other => Some(other),
        }
    }
}

struct Preparation {
    registry: Arc<ImmutableWorkerRegistry>,
    owner: String,
    ticket: String,
    cancel: CancellationToken,
}

impl Preparation {
    fn begin(registry: &Arc<ImmutableWorkerRegistry>, owner: &str) -> anyhow::Result<Self> {
        let mut state = registry
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let previous = match state.knowledge.get(owner) {
            Some(Slot::Preparing { .. }) => Err(anyhow::anyhow!(
                "Knowledge preparation is already in progress"
            )),
            Some(Slot::Ready(generation)) => Ok(Some(generation.clone())),
            None => Ok(None),
        }?;
        let ticket = uuid::Uuid::new_v4().to_string();
        let cancel = CancellationToken::new();
        state.knowledge.insert(
            owner.into(),
            Slot::Preparing {
                ticket: ticket.clone(),
                cancel: cancel.clone(),
                previous,
            },
        );
        Ok(Self {
            registry: registry.clone(),
            owner: owner.into(),
            ticket,
            cancel,
        })
    }

    fn publish(
        &self,
        generation: Arc<Generation>,
        workers: Vec<ImmutableWorker>,
    ) -> anyhow::Result<()> {
        let mut state = self
            .registry
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        match state
            .knowledge
            .get(&self.owner)
            .filter(|_| !self.cancel.is_cancelled())
        {
            Some(Slot::Preparing {
                ticket, previous, ..
            }) if ticket == &self.ticket => {
                previous.iter().for_each(|generation| {
                    generation
                        .freshness
                        .invalidate("Replaced by a new knowledge generation");
                });
                let registered = state.workers.entry(self.owner.clone()).or_default();
                registered.retain(|worker| worker.view().description.kind != "knowledge");
                registered.extend(workers);
                state
                    .knowledge
                    .insert(self.owner.clone(), Slot::Ready(generation));
                Ok(())
            }
            _ => Err(anyhow::anyhow!(
                "Knowledge preparation was cancelled or its conversation changed"
            )),
        }
    }
}

impl Drop for Preparation {
    fn drop(&mut self) {
        self.cancel.cancel();
        let mut state = self
            .registry
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let restored = state
            .knowledge
            .remove(&self.owner)
            .and_then(|slot| slot.rollback(&self.ticket));
        match restored {
            Some(slot) => {
                state.knowledge.insert(self.owner.clone(), slot);
            }
            None => {}
        }
    }
}

impl ImmutableWorkerRegistry {
    pub fn knowledge(&self, owner: &str) -> anyhow::Result<Arc<Generation>> {
        let state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        match state.knowledge.get(owner) {
            Some(
                Slot::Ready(generation)
                | Slot::Preparing {
                    previous: Some(generation),
                    ..
                },
            ) => Ok(generation.clone()),
            _ => Err(anyhow::anyhow!(
                "No knowledge generation is ready; explicitly prepare one first"
            )),
        }
    }

    pub async fn knowledge_status(
        &self,
        owner: &str,
        workspace: &WorkspacePolicy,
        budget: KnowledgeBudget,
        offset: usize,
        limit: usize,
    ) -> anyhow::Result<Status> {
        let page = WorkerPage::new(offset, limit)?;
        let slot = self
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .knowledge
            .get(owner)
            .cloned();
        match slot {
            Some(Slot::Preparing { ticket, .. }) => Ok(Status::Preparing { ticket }),
            Some(Slot::Ready(generation)) => generation.status(workspace, budget, page).await,
            None => Ok(Status::Absent),
        }
    }

    pub async fn prepare_knowledge(
        self: &Arc<Self>,
        owner: &str,
        profile: SemanticProfile,
        context: BuildContext<'_>,
    ) -> anyhow::Result<GenerationSummary> {
        let preparation = Preparation::begin(self, owner)?;
        tokio::select! {
            _ = preparation.cancel.cancelled() => Err(anyhow::anyhow!("Knowledge preparation cancelled")),
            result = async {
                let prepared = utils::knowledge::prepare(profile).await?;
                self.build_knowledge(&preparation, prepared, context).await
            } => result,
        }
    }

    pub async fn repartition_knowledge(
        self: &Arc<Self>,
        owner: &str,
        context: BuildContext<'_>,
    ) -> anyhow::Result<GenerationSummary> {
        let previous = self.knowledge(owner)?;
        let preparation = Preparation::begin(self, owner)?;
        let workspace = context.workspace.clone();
        let fingerprint = previous.freshness.fingerprint.clone();
        match tokio::task::spawn_blocking(move || fingerprint.is_current(&workspace)).await?? {
            true => Ok(()),
            false => Err(anyhow::anyhow!(
                "Sources changed; semantic preparation, not repartitioning, is required"
            )),
        }?;
        let prepared = PreparedKnowledge {
            graph: (*previous.index.graph).clone(),
            fingerprint: previous.freshness.fingerprint.clone(),
        };
        self.build_knowledge(&preparation, prepared, context).await
    }

    async fn build_knowledge(
        &self,
        preparation: &Preparation,
        prepared: PreparedKnowledge,
        context: BuildContext<'_>,
    ) -> anyhow::Result<GenerationSummary> {
        let freshness = Arc::new(Freshness::new(
            prepared.fingerprint,
            context.workspace.clone(),
            context.client,
        ));
        let client = context.client.snapshot();
        let budget = budget(&client, context.budget)?;
        let ticket = preparation.ticket.clone();
        let cancel = preparation.cancel.clone();
        let scope = ExecutionScope::current();
        let operation = move || {
            KnowledgeIndex::new(
                Arc::new(prepared.graph),
                ticket,
                budget,
                &|text| match cancel.is_cancelled() {
                    true => Err(anyhow::anyhow!("Knowledge partitioning cancelled")),
                    false => estimated_tokens(&request(text, budget)),
                },
            )
        };
        let index = scope.tasks.spawn_blocking(operation).await??;
        let mut workers = Vec::new();
        for shard in &index.shards {
            match preparation.cancel.is_cancelled() {
                true => Err(anyhow::anyhow!("Knowledge preparation cancelled")),
                false => Ok(()),
            }?;
            let state = KnowledgeWorkerState::new(
                &shard.context,
                &client,
                budget,
                context.timeout,
                freshness.clone(),
                self.knowledge_gate.clone(),
            )?;
            workers.push(ImmutableWorker::spawn(KnowledgeWorker, state, ImmutableWorkerDescription {
                kind: "knowledge".into(), description: format!("Repository knowledge generation {}, shard {}. {} primary source bytes across {} files; use knowledge search/inspect for routing. Fixed context, no tools; stale inputs require rebuilding.", index.generation, shard.summary.index, shard.summary.owned_bytes, shard.summary.paths.len()),
            }, context.actor).await?);
        }
        freshness.check().await?;
        let generation = Arc::new(Generation {
            index,
            freshness,
            workers: workers.iter().map(ImmutableWorker::view).collect(),
        });
        let summary = generation.summary(WorkerPage::new(0, 32)?);
        preparation.publish(generation, workers)?;
        Ok(summary)
    }
}

pub struct BuildContext<'a> {
    pub workspace: Arc<WorkspacePolicy>,
    pub client: &'a LLmClient,
    pub actor: &'a ActorRef<Message>,
    pub budget: ContextBudget,
    pub timeout: Duration,
}

#[cfg(test)]
mod tests;
