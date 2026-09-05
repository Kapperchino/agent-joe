use crate::tool_defs::{ToolDefTrait, ToolId, ToolTrait, ToolType};
use analysis::contexts::context::Context;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::cmp::min;
use std::fmt::{Display, Formatter};
use turbo_code_macros::{ToolDef, ToolInput};
use utils::files::Files;
use utils::utils::FnvHashMap;

#[async_trait]
impl<C: Context, A> ToolTrait<C, A> for InsertAfterLine {
    type Input = InsertAfterLineInput;
    type Output = InsertAfterLineResult;

    async fn run(
        input: Self::Input,
        tool_id: ToolId,
        _cur_context: &C,
        _actor_context: &A,
    ) -> anyhow::Result<Self::Output> {
        InsertAfterLine {
            input,
            id: String::new(),
        }
        .insert_after_line()
        .await?;

        Ok(InsertAfterLineResult {
            status: "ok".to_string(),
            id: tool_id,
        })
    }

    fn display_input(input: &Self::Input) -> String {
        InsertAfterLine {
            input: input.clone(),
            id: String::new(),
        }
        .to_string()
    }

    fn req_from_input(input: &Self::Input) -> anyhow::Result<FnvHashMap<String, String>> {
        InsertAfterLine {
            input: input.clone(),
            id: String::new(),
        }
        .req()
    }

    fn output_to_content(_input: &Self::Input, output: &Self::Output) -> anyhow::Result<String> {
        Ok(output.status.clone())
    }

    fn tool_type() -> ToolType {
        ToolType::Client
    }
}

#[derive(Default, Serialize, Deserialize, Debug, Clone, ToolDef)]
#[tool(name = "insert_after_line", description = "Insert content at line_num")]
pub struct InsertAfterLine {
    #[tool(input)]
    pub input: InsertAfterLineInput,
    pub id: String,
}

#[derive(Default, Serialize, Deserialize, Debug, Clone, ToolInput)]
pub struct InsertAfterLineInput {
    #[tool(description = "Content to insert after line line_num", required)]
    pub content: String,
    #[tool(description = "Path of the file to insert", required)]
    pub file_path: String,
    #[tool(description = "Line number of the file to insert to", required)]
    pub line_num: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InsertAfterLineResult {
    pub status: String,
    pub id: ToolId,
}

impl Display for InsertAfterLine {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "- edit `{}` after line {}",
            self.input.file_path, self.input.line_num
        )
    }
}

impl InsertAfterLine {
    async fn insert_after_line(&self) -> anyhow::Result<()> {
        let path = std::path::Path::new(&self.input.file_path);
        let content = Files::read_file(path).await?;
        let mut lines: Vec<_> = content.lines().collect();
        let line = min(self.input.line_num.saturating_sub(1), lines.len());
        lines.splice(line..line, self.input.content.lines());
        let mut content = lines.join("\n");
        content.push('\n');
        Files::write_to_file(path, &content).await
    }
}
