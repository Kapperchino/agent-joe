use crate::tool_defs::{ToolDefTrait, ToolId, ToolTrait, ToolType};
use analysis::contexts::context::Context;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};
use turbo_code_macros::{ToolDef, ToolInput};
use utils::utils::FnvHashMap;

#[async_trait]
impl<C: Context, A> ToolTrait<C, A> for WebSearch {
    type Input = WebSearchInput;
    type Output = WebSearchResult;

    async fn run(
        _input: Self::Input,
        _tool_id: ToolId,
        _cur_context: &C,
        _actor_context: &A,
    ) -> anyhow::Result<Self::Output> {
        Err(anyhow::anyhow!(
            "web_search is a provider-hosted tool and cannot be executed locally"
        ))
    }

    fn display_input(input: &Self::Input) -> String {
        WebSearch {
            input: input.clone(),
            id: String::new(),
        }
        .to_string()
    }

    fn req_from_input(input: &Self::Input) -> anyhow::Result<FnvHashMap<String, String>> {
        WebSearch {
            input: input.clone(),
            id: String::new(),
        }
        .req()
    }

    fn output_to_content(_input: &Self::Input, output: &Self::Output) -> anyhow::Result<String> {
        Ok(output.content.clone())
    }

    fn effect() -> crate::tool_defs::ToolEffect {
        crate::tool_defs::ToolEffect::Read
    }

    fn tool_type() -> ToolType {
        ToolType::Search
    }
}

#[derive(Default, Serialize, Deserialize, Debug, Clone, ToolDef)]
#[tool(
    name = "web_search",
    description = "Search the web for current information. This is executed by the LLM provider."
)]
pub struct WebSearch {
    #[tool(input)]
    pub input: WebSearchInput,
    pub id: String,
}

#[derive(Default, Serialize, Deserialize, Debug, Clone, ToolInput)]
pub struct WebSearchInput {}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebSearchResult {
    pub content: String,
    pub id: ToolId,
}

impl Display for WebSearch {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "- web search")
    }
}
