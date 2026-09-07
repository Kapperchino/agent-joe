use crate::tool_defs::{ToolDefTrait, ToolId, ToolTrait, ToolType};
use analysis::contexts::context::Context;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};
use turbo_code_macros::{ToolDef, ToolInput};
use utils::utils::FnvHashMap;

#[async_trait]
impl<C: Context, A> ToolTrait<C, A> for GrepTool {
    type Input = GrepInput;
    type Output = GrepResult;

    async fn run(
        input: Self::Input,
        tool_id: ToolId,
        cur_context: &C,
        _actor_context: &A,
    ) -> anyhow::Result<Self::Output> {
        let res = GrepTool {
            input,
            id: String::new(),
        }
        .grep(cur_context)
        .await?;

        Ok(GrepResult { res, id: tool_id })
    }

    fn display_input(input: &Self::Input) -> String {
        GrepTool {
            input: input.clone(),
            id: String::new(),
        }
        .to_string()
    }

    fn req_from_input(input: &Self::Input) -> anyhow::Result<FnvHashMap<String, String>> {
        GrepTool {
            input: input.clone(),
            id: String::new(),
        }
        .req()
    }

    fn output_to_content(_input: &Self::Input, output: &Self::Output) -> anyhow::Result<String> {
        Ok(output.res.clone())
    }

    fn effect() -> crate::tool_defs::ToolEffect {
        crate::tool_defs::ToolEffect::Read
    }

    fn tool_type() -> ToolType {
        ToolType::Client
    }
}

#[derive(Default, Serialize, Deserialize, Debug, Clone, ToolDef)]
#[tool(
    name = "grep",
    description = "Search all discoverable project text files, including manifests, CI, documentation and fixtures. Regex by default; optional literal mode and include/exclude globs. One-based lines, bounded results and explicit truncation metadata."
)]
pub struct GrepTool {
    #[tool(input)]
    pub input: GrepInput,
    pub id: String,
}

#[derive(Default, Serialize, Deserialize, Debug, Clone, ToolInput)]
pub struct GrepInput {
    #[tool(description = "The regex pattern to search for", required)]
    pub regex: String,
    #[tool(
        description = "Number of context lines to include before each match",
        required
    )]
    pub add_start: usize,
    #[tool(
        description = "Number of context lines to include after each match",
        required
    )]
    pub add_end: usize,
    #[tool(description = "Literal search instead of regex; default false")]
    pub literal: Option<bool>,
    #[tool(
        description = "Include workspace-relative globs separated by newlines; ** matches directories"
    )]
    pub include: Option<String>,
    #[tool(description = "Exclude path globs separated by newlines; excludes take priority")]
    pub exclude: Option<String>,
    #[tool(description = "Maximum matching lines, 1 to 1000; default 200")]
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrepResult {
    pub res: String,
    pub id: ToolId,
}

impl Display for GrepTool {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "- grep `{}` (before: {}, after: {})",
            self.input.regex, self.input.add_start, self.input.add_end
        )
    }
}

impl GrepTool {
    async fn grep<C: Context>(&self, cur_context: &C) -> anyhow::Result<String> {
        let query = utils::discovery::SearchQuery::new(
            &self.input.regex,
            if self.input.literal.unwrap_or(false) {
                utils::discovery::SearchMode::Literal
            } else {
                utils::discovery::SearchMode::Regex
            },
            self.input.include.as_deref().unwrap_or(""),
            self.input.exclude.as_deref().unwrap_or(""),
            self.input.limit,
            self.input.add_start,
            self.input.add_end,
        )?;
        let result = utils::files::operation(move |workspace| {
            query.search(
                workspace,
                utils::inventory::Inventory::scan(workspace)?,
                utils::discovery::SearchTarget::Text,
            )
        })
        .await?;
        let _ = cur_context;
        Ok(serde_json::to_string(&result)?)
    }
}
