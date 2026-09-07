use crate::tool_defs::{ToolDefTrait, ToolId, ToolTrait, ToolType};
use analysis::contexts::context::Context;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};
use turbo_code_macros::{ToolDef, ToolInput};
use utils::utils::FnvHashMap;

#[derive(Default, Serialize, Deserialize, Debug, Clone, ToolDef)]
#[tool(
    name = "list_directory",
    description = "List allowed files and directories, including ignored entries, in sorted bounded pages. This does not recursively traverse the directory. Protected and inaccessible entries are excluded."
)]
pub struct ListDirectory {
    #[tool(input)]
    pub input: ListDirectoryInput,
}

#[derive(Default, Serialize, Deserialize, Debug, Clone, ToolInput)]
pub struct ListDirectoryInput {
    #[tool(description = "Directory path relative to the project root", required)]
    pub path: String,
    #[tool(description = "Zero-based entry offset; default 0")]
    pub offset: Option<usize>,
    #[tool(description = "Maximum results, 1 to 1000; default 200")]
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListDirectoryOutput {
    pub result: serde_json::Value,
    pub id: ToolId,
}

impl Display for ListDirectory {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "- list directory `{}`", self.input.path)
    }
}

#[async_trait]
impl<C: Context, A> ToolTrait<C, A> for ListDirectory {
    type Input = ListDirectoryInput;
    type Output = ListDirectoryOutput;
    async fn run(
        input: Self::Input,
        tool_id: ToolId,
        context: &C,
        _actor: &A,
    ) -> anyhow::Result<Self::Output> {
        let limit = utils::inventory::ResultLimit::new(input.limit)?;
        let result = utils::files::operation(move |workspace| {
            utils::inventory::Listing::read(
                workspace,
                std::path::Path::new(&input.path),
                input.offset.unwrap_or(0),
                limit,
            )
        })
        .await?;
        let _ = context;
        let result = serde_json::to_value(result)?;
        Ok(ListDirectoryOutput {
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
