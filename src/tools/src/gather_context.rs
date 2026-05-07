use crate::tool_defs::{ToolDefTrait, ToolInputSchema};
use crate::tool_defs::{ToolId, ToolTrait};
use analysis::contexts::context::Context;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};
use turbo_code_macros::{ToolDef, ToolInput};

#[async_trait]
impl ToolTrait for GatherContext {
    type Input = GatherContextInput;
    type Output = GatherContextResult;

    async fn run<C: Context>(
        _input: Self::Input,
        _tool_id: ToolId,
        _cur_context: &C,
    ) -> anyhow::Result<Self::Output> {
        anyhow::bail!("gather_context tool is not implemented")
    }

    fn display_input(input: &Self::Input) -> String {
        GatherContext {
            input: input.clone(),
            id: String::new(),
        }
        .to_string()
    }

    fn req_from_input(
        input: &Self::Input,
    ) -> anyhow::Result<std::collections::HashMap<String, String>> {
        GatherContext {
            input: input.clone(),
            id: String::new(),
        }
        .req()
    }

    fn output_to_content(_input: &Self::Input, output: &Self::Output) -> anyhow::Result<String> {
        Ok(output.res.clone())
    }
}

#[derive(Default, Serialize, Deserialize, Debug, Clone, ToolDef)]
#[tool(
    name = "gather_context",
    description = "Create an agent to gather context"
)]
pub struct GatherContext {
    #[tool(input)]
    pub input: GatherContextInput,
    pub id: String,
}

#[derive(Default, Serialize, Deserialize, Debug, Clone, ToolInput)]
pub struct GatherContextInput {
    #[tool(description = "Context to give to the agent", required)]
    pub context: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatherContextResult {
    pub res: String,
    pub id: ToolId,
}

impl Display for GatherContext {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let context = self.input.context.trim();
        if context.is_empty() {
            write!(f, "- gather context")
        } else {
            let summary = context.lines().next().unwrap_or(context).trim();
            let summary: String = summary.chars().take(80).collect();
            if context.chars().count() > 80 {
                write!(f, "- gather context: `{summary}...`")
            } else {
                write!(f, "- gather context: `{summary}`")
            }
        }
    }
}
