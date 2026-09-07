use crate::tool_defs::{ToolDefTrait, ToolId, ToolTrait, ToolType};
use analysis::contexts::context::Context;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};
use turbo_code_macros::{ToolDef, ToolInput};
use utils::utils::FnvHashMap;

#[derive(Default, Serialize, Deserialize, Debug, Clone, ToolDef)]
#[tool(
    name = "find_files",
    description = "Search filenames and paths in a deterministic inventory of all discoverable files, including manifests, documentation, CI and fixtures. Honors .gitignore and .ignore. Returns truncation and skipped-entry metadata."
)]
pub struct FindFiles {
    #[tool(input)]
    pub input: FindFilesInput,
}

#[derive(Default, Serialize, Deserialize, Debug, Clone, ToolInput)]
pub struct FindFilesInput {
    #[tool(
        description = "Pattern matched against workspace-relative paths; empty lists all files",
        required
    )]
    pub pattern: String,
    #[tool(description = "Use literal matching instead of regex; default true")]
    pub literal: Option<bool>,
    #[tool(description = "Include path globs separated by newlines; ** matches directories")]
    pub include: Option<String>,
    #[tool(description = "Exclude path globs separated by newlines; excludes take priority")]
    pub exclude: Option<String>,
    #[tool(description = "Maximum results, 1 to 1000; default 200")]
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FindFilesOutput {
    pub result: serde_json::Value,
    pub id: ToolId,
}

impl Display for FindFiles {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "- find files `{}`", self.input.pattern)
    }
}

#[async_trait]
impl<C: Context, A> ToolTrait<C, A> for FindFiles {
    type Input = FindFilesInput;
    type Output = FindFilesOutput;
    async fn run(
        input: Self::Input,
        tool_id: ToolId,
        context: &C,
        _actor: &A,
    ) -> anyhow::Result<Self::Output> {
        let query = utils::discovery::SearchQuery::new(
            &input.pattern,
            if input.literal.unwrap_or(true) {
                utils::discovery::SearchMode::Literal
            } else {
                utils::discovery::SearchMode::Regex
            },
            input.include.as_deref().unwrap_or(""),
            input.exclude.as_deref().unwrap_or(""),
            input.limit,
            0,
            0,
        )?;
        let result = utils::files::operation(move |workspace| {
            let inventory = utils::inventory::Inventory::scan(workspace)?;
            query.search(workspace, inventory, utils::discovery::SearchTarget::Paths)
        })
        .await?;
        let _ = context;
        let result = serde_json::to_value(result)?;
        Ok(FindFilesOutput {
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
