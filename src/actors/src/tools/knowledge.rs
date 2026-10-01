use crate::{
    actor::{ActorContext, ActorInfo},
    knowledge::{BuildContext, budget},
    states::runtime::ExecutionRole,
};
use analysis::contexts::context::Context;
use async_trait::async_trait;
use common_models::knowledge::{
    ANALYZER_VERSION, Configuration, DefaultFeatures, Features, SemanticProfile, SourcePath,
    SymbolId,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeSet, time::Duration};
use tools::tool_defs::{LenientDeserialize, Range, ToolId, ToolOpKind, ToolTrait, ToolType};
use turbo_code_macros::{ToolDef, ToolSchema};
use utils::utils::FnvHashMap;

mod read;

#[derive(ToolDef)]
#[tool(
    name = "knowledge",
    description = "Gather repository and conversation context, not just file contents. For nontrivial or cross-module work: status checks generation readiness; prepare builds absent or source-stale Rust knowledge in implementation mode; search routes paths/symbols/documentation; inspect explains a returned symbol and relationships; ask queries relevant returned worker IDs with self-contained questions. Reuse current knowledge and carry generation for pagination and inspection. Use list then ask to recover missing historical context from immutable workers, especially after compaction. Read returns current UTF-8 content with one-based lines and exclusive range ends, or a bounded directory listing, and activates scoped instructions before editing. Read needs no preparation, including for new, non-Rust or explicitly named ignored files. Current knowledge adds up to eight related semantic source excerpts (2048 characters each), truncation metadata and worker IDs; unavailable knowledge never blocks allowed reads. Ask accepts worker_id from list/read/search/inspect/status; workers are tool-free, in-memory and retain no questions or answers. Answers are partial reference evidence, not fresh validation. Prepare runs locally in process without Cargo, build scripts, proc macros or provider requests, requires root Cargo.toml and whole-workspace root access, and is unavailable in plan mode. Sysroot, registry/git dependencies, generated code and custom cfg remain coverage gaps. Changed source requires prepare again; repartition reuses unchanged semantic inputs after model/budget changes. Clear intentionally retires repository knowledge only, preserving snapshots. All actions except read require the root conversation with whole-workspace access. If unavailable, report the limitation and use focused discovery and reads; do not claim semantic coverage. No startup indexing.",
    input = "Input"
)]
pub struct Knowledge;

#[derive(Clone, Debug, Deserialize, Serialize, ToolSchema)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum Input {
    Read {
        #[tool(nullable, description = "read only: known file or directory path")]
        file_path: String,
        #[tool(
            description = "read only: one-based start inclusive, end exclusive, clamped to EOF; omit for the whole file"
        )]
        range: Option<Range>,
    },
    List {},
    Ask {
        #[tool(
            nullable,
            description = "ask only: immutable worker ID from list/read/search/status"
        )]
        worker_id: String,
        #[tool(
            nullable,
            description = "ask only: independent nonempty question; respect the worker's max_question_bytes, at most 16384 UTF-8 bytes"
        )]
        question: String,
    },
    Prepare {
        #[tool(max_items = 128, description = "prepare only: explicit Cargo features")]
        features: Option<Vec<String>>,
        #[tool(description = "prepare only: defaults to enabled")]
        default_features: Option<DefaultFeatures>,
        #[tool(
            min_items = 1,
            max_items = 2,
            description = "prepare only: defaults to normal and test; not an all-cfg union"
        )]
        configurations: Option<Vec<Configuration>>,
    },
    Repartition,
    Status {
        #[tool(minimum = 0, description = "status/search only: default 0")]
        offset: Option<usize>,
        #[tool(
            minimum = 1,
            maximum = 32,
            description = "status/search only: default 10"
        )]
        limit: Option<usize>,
    },
    Search {
        #[tool(nullable, description = "search only: 1–32 words, at most 2048 bytes")]
        query: String,
        #[tool(
            description = "Required for inspect and nonzero search offsets; identity from status/search"
        )]
        generation: Option<String>,
        #[tool(minimum = 0, description = "status/search only: default 0")]
        offset: Option<usize>,
        #[tool(
            minimum = 1,
            maximum = 32,
            description = "status/search only: default 10"
        )]
        limit: Option<usize>,
    },
    Inspect {
        #[tool(nullable, description = "inspect only: semantic symbol ID from search")]
        symbol: String,
        #[tool(
            nullable,
            description = "Required for inspect and nonzero search offsets; identity from status/search"
        )]
        generation: String,
    },
    Clear,
}

impl LenientDeserialize for Input {
    fn deserialize_lenient(mut value: Value) -> anyhow::Result<Self> {
        if let Some(object) = value.as_object_mut() {
            object.retain(|_, value| !value.is_null());
        }
        Ok(serde_json::from_value(value)?)
    }
}

struct PreparationProfile {
    semantic: SemanticProfile,
}

impl PreparationProfile {
    fn new(
        features: Option<Vec<String>>,
        defaults: Option<DefaultFeatures>,
        configurations: Option<Vec<Configuration>>,
    ) -> anyhow::Result<Self> {
        let names: BTreeSet<_> = features.unwrap_or_default().into_iter().collect();
        let defaults = defaults.unwrap_or(DefaultFeatures::Enabled);
        let configurations: BTreeSet<_> = configurations
            .unwrap_or_else(|| vec![Configuration::Normal, Configuration::Test])
            .into_iter()
            .collect();
        match names.len() <= 128
            && names.iter().all(|name| {
                !name.is_empty() && name.len() <= 256 && !name.chars().any(char::is_whitespace)
            })
            && !configurations.is_empty()
        {
            true => Ok(()),
            false => Err(anyhow::anyhow!(
                "Select at least one configuration and at most 128 nonempty feature names without whitespace"
            )),
        }?;
        let features = match (names.is_empty(), defaults) {
            (true, DefaultFeatures::Enabled) => Features::Default,
            (true, DefaultFeatures::Disabled) => Features::None,
            (false, defaults) => Features::Named { names, defaults },
        };
        Ok(Self {
            semantic: SemanticProfile {
                manifest: SourcePath::try_from("Cargo.toml".to_owned())?,
                target: utils::knowledge::native_target(),
                features,
                configurations,
                analyzer_version: ANALYZER_VERSION.into(),
            },
        })
    }
}

struct SearchRequest {
    query: String,
    generation: Option<String>,
    offset: usize,
    limit: usize,
}

impl SearchRequest {
    fn new(
        query: String,
        generation: Option<String>,
        offset: Option<usize>,
        limit: Option<usize>,
    ) -> anyhow::Result<Self> {
        match limit.unwrap_or(10) {
            limit @ 1..=32 => Ok(Self {
                query,
                generation,
                offset: offset.unwrap_or(0),
                limit,
            }),
            _ => Err(anyhow::anyhow!("Knowledge search requires a limit of 1–32")),
        }
    }
}

pub(crate) fn access<C: Context>(actor: &ActorContext<C>) -> anyhow::Result<&ActorInfo<C>> {
    match actor {
        ActorContext::ActorInfo(info)
            if matches!(info.runtime.role, ExecutionRole::Root)
                && info
                    .runtime
                    .scope
                    .workspace()?
                    .permits_workspace_access(utils::workspace::Access::Read) =>
        {
            Ok(info)
        }
        _ => Err(anyhow::anyhow!(
            "Repository knowledge requires the root conversation with whole-workspace access"
        )),
    }
}

impl std::fmt::Display for Knowledge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "knowledge")
    }
}

impl Knowledge {
    async fn repository<C: Context>(
        input: Input,
        actor: &ActorContext<C>,
    ) -> anyhow::Result<Value> {
        let info = access(actor)?;
        let registry = &info.runtime.immutable_workers;
        let workspace = &info.runtime.scope.workspace()?;
        let context = BuildContext {
            workspace: workspace.clone(),
            client: &info.services.client,
            actor: &info.actor_ref,
            budget: info.runtime.context_budget,
            timeout: info.runtime.request_timeout,
        };
        match input {
            Input::Read { .. } | Input::List {} | Input::Ask { .. } => {
                Err(anyhow::anyhow!("Expected a repository knowledge operation"))
            }
            Input::Prepare {
                features,
                default_features,
                configurations,
            } => {
                budget(&info.services.client, info.runtime.context_budget)?;
                let profile = PreparationProfile::new(features, default_features, configurations)?;
                Ok(serde_json::to_value(
                    registry
                        .prepare_knowledge(&info.owner, profile.semantic, context)
                        .await?,
                )?)
            }
            Input::Repartition => Ok(serde_json::to_value(
                registry.repartition_knowledge(&info.owner, context).await?,
            )?),
            Input::Status { offset, limit } => Ok(serde_json::to_value(
                registry
                    .knowledge_status(
                        &info.owner,
                        workspace,
                        budget(&info.services.client, info.runtime.context_budget)?,
                        offset.unwrap_or(0),
                        limit.unwrap_or(10),
                    )
                    .await?,
            )?),
            Input::Clear => {
                registry.clear_knowledge(&info.owner);
                Ok(json!({"state":"absent"}))
            }
            Input::Search {
                query,
                generation,
                offset,
                limit,
            } => {
                let budget = budget(&info.services.client, info.runtime.context_budget)?;
                let request = SearchRequest::new(query, generation, offset, limit)?;
                let knowledge = registry.knowledge(&info.owner)?;
                knowledge.check(workspace, budget).await?;
                let result = knowledge.search(
                    &request.query,
                    request.generation.as_deref(),
                    request.offset,
                    request.limit,
                )?;
                knowledge.check(workspace, budget).await?;
                Ok(serde_json::to_value(result)?)
            }
            Input::Inspect { symbol, generation } => {
                let budget = budget(&info.services.client, info.runtime.context_budget)?;
                let knowledge = registry.knowledge(&info.owner)?;
                knowledge.check(workspace, budget).await?;
                let result = knowledge.inspect(&SymbolId(symbol), &generation)?;
                knowledge.check(workspace, budget).await?;
                Ok(serde_json::to_value(result)?)
            }
        }
    }
}

#[async_trait]
impl<C: Context> ToolTrait<C, ActorContext<C>> for Knowledge {
    type Input = Input;
    type Output = Value;

    async fn run(
        input: Input,
        id: ToolId,
        context: &C,
        actor: &ActorContext<C>,
    ) -> anyhow::Result<Value> {
        use super::ask_immutable_worker::{AskImmutableWorker, AskImmutableWorkerInput};
        match input {
            Input::Read { file_path, range } => read::run(file_path, range, context, actor).await,
            Input::List {} => {
                access(actor)?;
                Ok(serde_json::to_value(
                    AskImmutableWorker::run(
                        AskImmutableWorkerInput {
                            action: "list".into(),
                            worker_id: None,
                            question: None,
                        },
                        id,
                        context,
                        actor,
                    )
                    .await?,
                )?)
            }
            Input::Ask {
                worker_id,
                question,
            } => {
                access(actor)?;
                Ok(serde_json::to_value(
                    AskImmutableWorker::run(
                        AskImmutableWorkerInput {
                            action: "ask".into(),
                            worker_id: Some(worker_id),
                            question: Some(question),
                        },
                        id,
                        context,
                        actor,
                    )
                    .await?,
                )?)
            }
            input => Self::repository(input, actor).await,
        }
    }

    fn display_input(input: &Input) -> String {
        match input {
            Input::Read { file_path, range } => format!("- knowledge read `{file_path}` {range:?}"),
            Input::Ask { worker_id, .. } => format!("- knowledge ask {worker_id}"),
            _ => format!("knowledge {input:?}"),
        }
    }
    fn req_from_input(_: &Input) -> anyhow::Result<FnvHashMap<String, String>> {
        Ok(Default::default())
    }
    fn output_to_content(_: &Input, output: &Value) -> anyhow::Result<String> {
        Ok(serde_json::to_string(output)?)
    }
    fn tool_type() -> ToolType {
        ToolType::Client
    }
    fn effect() -> ToolOpKind {
        ToolOpKind::Read
    }
    fn effect_from_input(input: &Input) -> ToolOpKind {
        match input {
            Input::Prepare { .. } => ToolOpKind::Validate,
            Input::Ask { .. } => ToolOpKind::DelegateRead,
            _ => ToolOpKind::Read,
        }
    }
    fn execution_budget(input: &Input) -> anyhow::Result<Duration> {
        Ok(match input {
            Input::Prepare { .. } => Duration::from_secs(1800),
            _ => Duration::ZERO,
        })
    }
}

#[cfg(test)]
mod tests;
