use crate::tool_defs::{ToolDefTrait, ToolId, ToolTrait, ToolType};
use analysis::contexts::context::Context;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};
use turbo_code_macros::{ToolDef, ToolInput};
use utils::utils::FnvHashMap;

#[derive(Default, Serialize, Deserialize, Debug, Clone, ToolDef)]
#[tool(
    name = "inspect_context",
    description = "Inspect active AGENTS.md sources, their scopes and precedence, plus inventory and truncation metadata. An optional path activates its applicable nested instructions for the next request before editing."
)]
pub struct InspectContext {
    #[tool(input)]
    pub input: InspectContextInput,
}

#[derive(Default, Serialize, Deserialize, Debug, Clone, ToolInput)]
pub struct InspectContextInput {
    #[tool(
        description = "Optional file or directory whose scoped instructions should be activated"
    )]
    pub path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InspectContextOutput {
    pub result: serde_json::Value,
    pub id: ToolId,
}

impl Display for InspectContext {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "- inspect context")
    }
}

#[async_trait]
impl<C: Context, A> ToolTrait<C, A> for InspectContext {
    type Input = InspectContextInput;
    type Output = InspectContextOutput;
    async fn run(
        input: Self::Input,
        tool_id: ToolId,
        context: &C,
        _actor: &A,
    ) -> anyhow::Result<Self::Output> {
        if let Some(path) = input.path {
            context.discover_instructions(&[path.into()])?;
        }
        let result = serde_json::from_str(&context.inspect_context().await?)?;
        Ok(InspectContextOutput {
            result,
            id: tool_id,
        })
    }
    fn display_input(input: &Self::Input) -> String {
        Self {
            input: input.clone(),
        }
        .to_string()
    }
    fn req_from_input(input: &Self::Input) -> anyhow::Result<FnvHashMap<String, String>> {
        Self {
            input: input.clone(),
        }
        .req()
    }
    fn output_to_content(_input: &Self::Input, output: &Self::Output) -> anyhow::Result<String> {
        Ok(serde_json::to_string(&output.result)?)
    }
    fn effect() -> crate::tool_defs::ToolEffect {
        crate::tool_defs::ToolEffect::Read
    }
    fn tool_type() -> ToolType {
        ToolType::Client
    }
}
