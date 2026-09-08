use crate::{actor::ActorContext, worker_registry::report::WorkerReport};
use analysis::contexts::rust_context::RustContext;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tools::tool_defs::{ToolDefTrait, ToolId, ToolTrait, ToolType};
use turbo_code_macros::{ToolDef, ToolInput};
use utils::utils::FnvHashMap;

#[derive(Default, Debug, Clone, Serialize, Deserialize, ToolDef)]
#[tool(
    name = "validate_rust",
    description = "Run one bounded worker and await its structured report. Prefer direct tools for small work; start_worker supports explicit tool/path limits and asynchronous coordination."
)]
pub struct ValidateRust {
    #[tool(input)]
    pub input: ValidateRustInput,
    pub id: String,
}

#[derive(Default, Debug, Clone, Serialize, Deserialize, ToolInput)]
pub struct ValidateRustInput {
    #[tool(
        description = "A bounded objective, selected context, constraints, completion criteria and requested checks",
        required
    )]
    pub context: String,
}

pub type ValidateRustResult = WorkerReport;

impl std::fmt::Display for ValidateRust {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "- validate_rust: {}",
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
impl ToolTrait<RustContext, ActorContext<RustContext>> for ValidateRust {
    type Input = ValidateRustInput;
    type Output = ValidateRustResult;
    async fn run(
        input: Self::Input,
        _: ToolId,
        context: &RustContext,
        actor: &ActorContext<RustContext>,
    ) -> anyhow::Result<Self::Output> {
        super::delegated::run(input.context, "cargo", context, actor).await
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
        tools::tool_defs::ToolEffect::DelegateValidate
    }
    fn tool_type() -> ToolType {
        ToolType::Client
    }
}
