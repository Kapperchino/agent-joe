use crate::tool_defs::{ToolDefTrait, ToolId, ToolTrait, ToolType};
use analysis::contexts::context::Context;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};
use std::path::PathBuf;
use turbo_code_macros::{ToolDef, ToolInput};
use utils::grep::Grep as ProjectGrep;
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

    fn tool_type() -> ToolType {
        ToolType::Client
    }
}

#[derive(Default, Serialize, Deserialize, Debug, Clone, ToolDef)]
#[tool(
    name = "grep",
    description = "Search the current project files with a regex and return matching lines with surrounding context"
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
        let mut file_paths: Vec<PathBuf> = cur_context.get_files().await?;
        file_paths.sort();
        let matches = ProjectGrep::grep(
            &self.input.regex,
            file_paths,
            self.input.add_start,
            self.input.add_end,
        )
        .await?;

        if matches.is_empty() {
            Ok("No matches found".to_string())
        } else {
            Ok(matches
                .into_iter()
                .map(|grep_match| grep_match.to_string())
                .collect::<Vec<_>>()
                .join("\n\n"))
        }
    }
}
