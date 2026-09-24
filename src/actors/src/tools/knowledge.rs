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
use tools::tool_defs::{LenientDeserialize, ToolId, ToolOpKind, ToolTrait, ToolType};
use turbo_code_macros::{ToolDef, ToolSchema};
use utils::utils::FnvHashMap;

#[derive(ToolDef)]
#[tool(
    name = "knowledge",
    description = "Prepare and query immutable repository knowledge actors. Explicit prepare uses rust-analyzer libraries in process over captured source and Cargo manifests to resolve local Rust references, calls and trait relations. No executable, Cargo, build script or proc macro is run. Requires root Cargo.toml, not Cargo.lock or a provisioned sandbox. Only captured local dependencies and native baseline cfg are analyzed; sysroot, registry/git dependencies, generated code and custom build cfg are diagnostic coverage gaps. Preparation is explicit and unavailable in plan mode. Status, search, inspect and repartition are read-only. Search routes paths/symbols/documentation to primary and related shard worker IDs; ask those IDs with ask_immutable_worker. Use generation for pagination and inspection. Repartition reuses unchanged semantic inputs after a model/budget change. Edits require prepare again. Clear retires knowledge only, preserving compaction snapshots. No startup indexing or provider requests during preparation. In-memory and scoped to this conversation.",
    input = "Input"
)]
pub struct Knowledge;

#[derive(Clone, Debug, Deserialize, Serialize, ToolSchema)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum Input {
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

fn profile(
    features: Option<Vec<String>>,
    defaults: Option<DefaultFeatures>,
    configurations: Option<Vec<Configuration>>,
) -> anyhow::Result<SemanticProfile> {
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
    Ok(SemanticProfile {
        manifest: SourcePath::try_from("Cargo.toml".to_owned())?,
        target: utils::knowledge::native_target(),
        features,
        configurations,
        analyzer_version: ANALYZER_VERSION.into(),
    })
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

#[async_trait]
impl<C: Context> ToolTrait<C, ActorContext<C>> for Knowledge {
    type Input = Input;
    type Output = Value;

    async fn run(input: Input, _: ToolId, _: &C, actor: &ActorContext<C>) -> anyhow::Result<Value> {
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
            Input::Prepare {
                features,
                default_features,
                configurations,
            } => {
                budget(&info.services.client, info.runtime.context_budget)?;
                let profile = profile(features, default_features, configurations)?;
                Ok(serde_json::to_value(
                    registry
                        .prepare_knowledge(&info.owner, profile, context)
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
                let limit = limit.unwrap_or(10);
                match (1..=32).contains(&limit) {
                    true => Ok(()),
                    false => Err(anyhow::anyhow!("Knowledge search requires a limit of 1–32")),
                }?;
                let knowledge = registry.knowledge(&info.owner)?;
                knowledge.check(workspace, budget).await?;
                let result =
                    knowledge.search(&query, generation.as_deref(), offset.unwrap_or(0), limit)?;
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

    fn display_input(input: &Input) -> String {
        format!("knowledge {input:?}")
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
