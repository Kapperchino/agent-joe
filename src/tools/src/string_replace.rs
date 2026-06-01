use crate::tool_defs::{ToolDefTrait, ToolId, ToolTrait, ToolType};
use analysis::contexts::context::Context;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};
use turbo_code_macros::{ToolDef, ToolInput};
use utils::text_search::TextSearch;

#[async_trait]
impl<C: Context, A> ToolTrait<C, A> for StringReplace {
    type Input = StringReplaceInput;
    type Output = StringReplaceResult;

    async fn run(
        input: Self::Input,
        tool_id: ToolId,
        _cur_context: &C,
        _actor_context: &A,
    ) -> anyhow::Result<Self::Output> {
        StringReplace {
            input,
            id: String::new(),
        }
        .str_replace()
        .await?;

        Ok(StringReplaceResult {
            status: "ok".to_string(),
            id: tool_id,
        })
    }

    fn display_input(input: &Self::Input) -> String {
        StringReplace {
            input: input.clone(),
            id: String::new(),
        }
        .to_string()
    }

    fn req_from_input(
        input: &Self::Input,
    ) -> anyhow::Result<std::collections::HashMap<String, String>> {
        StringReplace {
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
#[tool(name = "str_replace", description = "Replace a string with another")]
pub struct StringReplace {
    #[tool(input)]
    pub input: StringReplaceInput,
    pub id: String,
}

#[derive(Default, Serialize, Deserialize, Debug, Clone, ToolInput)]
pub struct StringReplaceInput {
    #[tool(description = "The exact text to find", required)]
    pub old_str: String,
    #[tool(description = "The text to replace it with", required)]
    pub new_str: String,
    #[tool(description = "Path of the file", required)]
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StringReplaceResult {
    pub status: String,
    pub id: ToolId,
}

impl Display for StringReplace {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "- replace text in `{}`", self.input.path)
    }
}

impl StringReplace {
    async fn str_replace(&self) -> anyhow::Result<()> {
        let StringReplaceInput {
            old_str,
            new_str,
            path,
        } = &self.input;
        TextSearch::search_and_replace(old_str, new_str, &(path.into())).await?;
        Ok(())
    }
}
