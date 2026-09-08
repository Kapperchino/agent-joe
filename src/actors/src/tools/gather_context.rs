use crate::{actor::ActorContext, worker_registry::report::WorkerReport};
use analysis::contexts::rust_context::RustContext;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tools::tool_defs::{ToolDefTrait, ToolId, ToolTrait, ToolType};
use turbo_code_macros::{ToolDef, ToolInput};
use utils::utils::FnvHashMap;

#[derive(Default, Debug, Clone, Serialize, Deserialize, ToolDef)]
#[tool(
    name = "gather_context",
    description = "Run one bounded worker and await its structured report. Prefer direct tools for small work; start_worker supports explicit tool/path limits and asynchronous coordination."
)]
pub struct GatherContext {
    #[tool(input)]
    pub input: GatherContextInput,
    pub id: String,
}

#[derive(Default, Debug, Clone, Serialize, Deserialize, ToolInput)]
pub struct GatherContextInput {
    #[tool(
        description = "A bounded objective, selected context, constraints, completion criteria and requested checks",
        required
    )]
    pub context: String,
}

pub type GatherContextResult = WorkerReport;

impl std::fmt::Display for GatherContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "- gather_context: {}",
            self.input
                .context
                .lines()
                .next()
                .unwrap_or_default()
                .chars()
                .take(80)
                .collect::<String>()
        )
    }
}

#[async_trait]
impl ToolTrait<RustContext, ActorContext<RustContext>> for GatherContext {
    type Input = GatherContextInput;
    type Output = GatherContextResult;
    async fn run(
        input: Self::Input,
        _: ToolId,
        context: &RustContext,
        actor: &ActorContext<RustContext>,
    ) -> anyhow::Result<Self::Output> {
        super::delegated::run(
            input.context,
            "find_files\nlist_directory\nread_file\ngrep",
            context,
            actor,
        )
        .await
    }
    fn display_input(input: &Self::Input) -> String {
        Self {
            input: input.clone(),
            id: String::new(),
        }
        .to_string()
    }
    fn req_from_input(input: &Self::Input) -> anyhow::Result<FnvHashMap<String, String>> {
        Self {
            input: input.clone(),
            id: String::new(),
        }
        .req()
    }
    fn output_to_content(_: &Self::Input, output: &Self::Output) -> anyhow::Result<String> {
        Ok(serde_json::to_string(output)?)
    }
    fn effect() -> tools::tool_defs::ToolEffect {
        tools::tool_defs::ToolEffect::DelegateRead
    }
    fn tool_type() -> ToolType {
        ToolType::Client
    }
}
