use crate::tool_defs::{Range, ToolDefTrait, ToolId, ToolTrait, ToolType};
use analysis::contexts::context::Context;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};
use std::path::PathBuf;
use turbo_code_macros::{ToolDef, ToolInput};
use utils::{files::Files, utils::FnvHashMap};

mod line_range;
use line_range::LineRange;

#[async_trait]
impl<C: Context, A> ToolTrait<C, A> for ReadFile {
    type Input = ReadFileInput;
    type Output = ReadFileResult;

    async fn run(
        input: Self::Input,
        tool_id: ToolId,
        cur_context: &C,
        _actor_context: &A,
    ) -> anyhow::Result<Self::Output> {
        let res = ReadFile {
            input,
            id: String::new(),
        }
        .read_file(cur_context)
        .await?;
        Ok(ReadFileResult { res, id: tool_id })
    }

    fn display_input(input: &Self::Input) -> String {
        ReadFile {
            input: input.clone(),
            id: String::new(),
        }
        .to_string()
    }

    fn req_from_input(input: &Self::Input) -> anyhow::Result<FnvHashMap<String, String>> {
        ReadFile {
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
    name = "read_file",
    description = r#"
Read contents from a specific known text file.

Reads any allowed UTF-8 file directly from disk, including new, non-Rust, and ignored files. Use find_files or grep to discover paths. Directories return a bounded first page; use list_directory for more pages.

Prefer a focused line range when the relevant location is known. Omit `range` only for small files or when full-file context is necessary.

Before editing, read the relevant file region unless that exact region is already present in current context.

Prefer parallel read calls.
"#
)]
pub struct ReadFile {
    #[tool(input)]
    pub input: ReadFileInput,
    pub id: String,
}

#[derive(Default, Serialize, Deserialize, Debug, Clone, ToolInput)]
pub struct ReadFileInput {
    #[tool(description = "file path of the file you want to read", required)]
    pub file_path: String,
    #[tool(
        description = "One-based start (inclusive) and end (exclusive). End is clamped to EOF. Omit to read the entire file."
    )]
    pub range: Option<Range>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadFileResult {
    pub res: String,
    pub id: ToolId,
}

impl Display for ReadFile {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match &self.input.range {
            Some(range) => write!(
                f,
                "- read `{}` (lines {}-{})",
                self.input.file_path,
                range.start,
                range.end.saturating_sub(1)
            ),
            None => write!(f, "- read `{}`", self.input.file_path),
        }
    }
}

impl ReadFile {
    pub async fn read_file<C: Context>(&self, cur_context: &C) -> anyhow::Result<String> {
        let path = PathBuf::from(&self.input.file_path);
        cur_context.discover_instructions(std::slice::from_ref(&path))?;
        match Files::is_directory(&path).await? {
            true => {
                utils::files::operation(move |workspace| {
                    Ok(serde_json::to_string(&utils::inventory::Listing::read(
                        workspace,
                        &path,
                        0,
                        utils::inventory::ResultLimit::new(None)?,
                    )?)?)
                })
                .await
            }
            false => match &self.input.range {
                Some(range) => Self::read_range(&path, range.clone(), cur_context).await,
                None => Files::read_file(&path).await.map(|text| {
                    text.lines()
                        .enumerate()
                        .map(|(index, line)| format!("{}: {line}", index + 1))
                        .collect::<Vec<_>>()
                        .join("\n")
                }),
            },
        }
    }

    async fn read_range<C: Context>(path: &PathBuf, range: Range, _: &C) -> anyhow::Result<String> {
        let range = LineRange::try_from(range)?;
        let text = Files::read_file(path).await?;
        range.render(&text)
    }
}

#[cfg(test)]
#[path = "../tests/unit/read_file/tests.rs"]
mod tests;
