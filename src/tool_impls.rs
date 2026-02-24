use crate::analysis::Range;
use crate::cargo::Cargo;
use crate::claude::ToolSchemaDTO;
use crate::cur_context::CurContext;
use crate::text_search::TextSearch;
use crate::tool_defs::CargoCheckResult;
pub(crate) use crate::tool_defs::{CargoCheck, InsertAfterLine, ReadFile, Tool, ToolResult};
pub(crate) use crate::tool_defs::{StringReplace, StringReplaceInput, ToolDefTrait};
use crate::{cargo, claude};
use anyhow::{anyhow, Error};
use futures::{StreamExt, TryStreamExt};
use itertools::Itertools;
use ra_ap_ide::TextSize;
use std::cmp::min;
use std::collections::HashMap;
use std::io::SeekFrom;
use tokio::fs;
use tokio::fs::File;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncSeekExt, BufReader};
use tokio_stream::wrappers::LinesStream;

impl Tool {
    pub fn name(&self) -> String {
        match self {
            Tool::ReadFile(_) => ReadFile::tool_name().to_string(),
            Tool::InsertAfterLine(_) => InsertAfterLine::tool_name().to_string(),
            Tool::StringReplace(_) => StringReplace::tool_name().to_string(),
            Tool::CargoCheck(_) => CargoCheck::tool_name().to_string(),
        }
    }

    pub fn id(&self) -> String {
        match self {
            Tool::ReadFile(file) => file.id.clone(),
            Tool::InsertAfterLine(insert) => insert.id.clone(),
            Tool::StringReplace(replace) => replace.id.clone(),
            Tool::CargoCheck(check) => check.id.clone(),
        }
    }

    pub fn to_json(&self) -> claude::Tool {
        claude::Tool {
            name: self.name(),
            description: match self {
                Tool::ReadFile(_) => ReadFile::tool_description().to_string(),
                Tool::InsertAfterLine(_) => InsertAfterLine::tool_description().to_string(),
                Tool::StringReplace(_) => StringReplace::tool_description().to_string(),
                Tool::CargoCheck(_) => CargoCheck::tool_description().to_string(),
            },
            input_schema: ToolSchemaDTO {
                name: self.name(),
                tool_type: "object".to_string(),
                properties: match self {
                    Tool::ReadFile(_) => ReadFile::field_properties(),
                    Tool::InsertAfterLine(_) => InsertAfterLine::field_properties(),
                    Tool::StringReplace(_) => StringReplace::field_properties(),
                    Tool::CargoCheck(_) => CargoCheck::field_properties(),
                },
                required: match self {
                    Tool::ReadFile(_) => ReadFile::required_fields(),
                    Tool::InsertAfterLine(_) => InsertAfterLine::required_fields(),
                    Tool::StringReplace(_) => StringReplace::required_fields(),
                    Tool::CargoCheck(_) => CargoCheck::required_fields(),
                },
            },
        }
    }

    pub fn to_req(&self) -> anyhow::Result<HashMap<String, String>> {
        match self {
            Tool::ReadFile(path) => path.req(),
            Tool::InsertAfterLine(insert) => insert.req(),
            Tool::StringReplace(replace) => replace.req(),
            Tool::CargoCheck(check) => check.req(),
        }
    }

    pub async fn use_tool(&self, id: String, ctx: &CurContext) -> anyhow::Result<ToolResult> {
        match self {
            Tool::ReadFile(read_file) => {
                let result = read_file.read_file(ctx).await?;
                Ok(ToolResult::ReadFileResult {
                    res: result,
                    tool: self.clone(),
                    id,
                })
            }
            Tool::InsertAfterLine(insert) => {
                insert.insert_after_line().await?;
                Ok(ToolResult::InsertAfterLineResult {
                    status: "ok".to_string(),
                    tool: self.clone(),
                    id,
                })
            }
            Tool::StringReplace(replace) => {
                replace.str_replace().await?;
                Ok(ToolResult::InsertAfterLineResult {
                    status: "ok".to_string(),
                    tool: self.clone(),
                    id,
                })
            }
            Tool::CargoCheck(check) => {
                let res = check.cargo_check().await?;
                Ok(ToolResult::CargoCheckResult {
                    status: match res {
                        CargoCheckResult::Success(_) => "success",
                        CargoCheckResult::Failed { .. } => "failed",
                    }
                    .to_string(),
                    result: res,
                    tool: self.clone(),
                    id,
                })
            }
        }
    }

    pub fn from_str(name: &str) -> anyhow::Result<Self> {
        match name {
            "read_file" => Ok(Tool::ReadFile(ReadFile::default())),
            "insert_after_line" => Ok(Tool::InsertAfterLine(InsertAfterLine::default())),
            "str_replace" => Ok(Tool::StringReplace(StringReplace::default())),
            "cargo_check" => Ok(Tool::CargoCheck(CargoCheck::default())),
            _ => Err(Error::msg("Is not a tool")),
        }
    }
}

impl ToolResult {
    pub fn tool(&self) -> Tool {
        match self {
            ToolResult::ReadFileResult { tool, .. } => tool.clone(),
            ToolResult::InsertAfterLineResult { tool, .. } => tool.clone(),
            ToolResult::StringReplaceResult { tool, .. } => tool.clone(),
            ToolResult::CargoCheckResult { tool, .. } => tool.clone(),
            ToolResult::Error { tool, .. } => tool.clone(),
        }
    }

    pub fn to_res_json(&self) -> claude::ContentBlock {
        match self {
            ToolResult::ReadFileResult { res, id, .. } => claude::ContentBlock::ToolResult {
                tool_use_id: id.to_string(),
                content: res.to_string(),
                is_error: None,
            },
            ToolResult::InsertAfterLineResult { status, id, .. } => {
                claude::ContentBlock::ToolResult {
                    tool_use_id: id.to_string(),
                    content: status.to_string(),
                    is_error: None,
                }
            }
            ToolResult::StringReplaceResult { status, id, .. } => {
                claude::ContentBlock::ToolResult {
                    tool_use_id: id.to_string(),
                    content: status.to_string(),
                    is_error: None,
                }
            }
            ToolResult::CargoCheckResult {
                status,
                result,
                id,
                tool,
            } => match result {
                CargoCheckResult::Success(warnings) => {
                    if let Tool::CargoCheck(cargo) = tool
                        && cargo.input.include_warnings.unwrap_or(false)
                    {
                        claude::ContentBlock::ToolResult {
                            tool_use_id: id.to_string(),
                            content: format!("{}\nWarnings:\n{}", status, warnings.join("\n")),
                            is_error: None,
                        }
                    } else {
                        claude::ContentBlock::ToolResult {
                            tool_use_id: id.to_string(),
                            content: status.clone(),
                            is_error: None,
                        }
                    }
                }
                CargoCheckResult::Failed { warnings, errors } => {
                    if let Tool::CargoCheck(cargo) = tool
                        && cargo.input.include_warnings.unwrap_or(false)
                    {
                        claude::ContentBlock::ToolResult {
                            tool_use_id: id.to_string(),
                            content: format!(
                                "{}\nWarnings:\n{}\n{}",
                                status,
                                warnings.join("\n"),
                                errors.join("\n")
                            ),
                            is_error: None,
                        }
                    } else {
                        claude::ContentBlock::ToolResult {
                            tool_use_id: id.to_string(),
                            content: format!("{}\nErrors:\n{}", status, warnings.join("\n")),
                            is_error: None,
                        }
                    }
                }
            },
            ToolResult::Error { message, id, .. } => claude::ContentBlock::ToolResult {
                tool_use_id: id.to_string(),
                content: message.clone(),
                is_error: Some(true),
            },
        }
    }
}

impl ReadFile {
    pub async fn read_file(&self, cur_context: &CurContext) -> anyhow::Result<String> {
        match &self.input.range {
            None => Self::read_entire_file(&self.input.file_path).await,
            Some(range) => {
                Self::read_range(&self.input.file_path, range.clone(), cur_context).await
            }
        }
    }
    async fn read_entire_file(file_path: &String) -> anyhow::Result<String> {
        let file = File::open(file_path).await?;
        let reader = BufReader::new(file);

        let lines = LinesStream::new(reader.lines());

        let res = lines
            .enumerate()
            .map(|(line, res)| res.map(|l_content| format!("{line}: {l_content}")))
            .try_fold(String::new(), |acc, line| async move {
                if acc.is_empty() {
                    Ok(format!("{line}"))
                } else {
                    Ok(format!("{acc}\n{line}"))
                }
            })
            .await?;

        Ok(res)
    }
    async fn read_range(
        file_path: &String,
        range: Range,
        cur_context: &CurContext,
    ) -> anyhow::Result<String> {
        let meta = cur_context.get_proj_meta().await?;
        match meta.files.get(file_path) {
            Some(meta) => {
                let start_line = meta.line_index.line_col(TextSize::new(range.start)).line;
                let Range { start, end } = range;
                let mut file = File::open(file_path).await?;
                file.seek(SeekFrom::Start(start as u64)).await?;
                let file_size = file.metadata().await?.len();
                let remaining: usize = (file_size - file.stream_position().await?) as usize;
                let buf_size = min(remaining, (end - start) as usize);
                let mut buf = vec![0; buf_size];
                file.read_exact(&mut buf).await?;
                let res = String::from_utf8(buf)?;
                let res = res
                    .lines()
                    .enumerate()
                    .fold(String::new(), |acc, (i, line)| {
                        let i = start_line + i as u32 + 1;
                        if acc.is_empty() {
                            format!("{i}: {line}")
                        } else {
                            format!("{acc}\n{i}: {line}")
                        }
                    });
                Ok(res)
            }
            None => Err(anyhow!("File not found!")),
        }
    }
}

impl InsertAfterLine {
    async fn insert_after_line(&self) -> anyhow::Result<()> {
        // line number starts at 1 for the agent
        let line_num = self.input.line_num.clone() - 1;
        let path = self.input.file_path.clone();
        let insert_lines: Vec<_> = self.input.content.lines().collect();
        let file_content = fs::read_to_string(&path).await?;
        let mut lines: Vec<_> = file_content.lines().collect();
        lines.splice(line_num..line_num, insert_lines);
        let mut res = lines.join("\n");
        res.push_str("\n");
        fs::write(&path, res).await?;
        Ok(())
    }
}

impl StringReplace {
    async fn str_replace(&self) -> anyhow::Result<()> {
        let StringReplaceInput {
            old_str,
            new_str,
            path,
        } = &self.input;
        TextSearch::search_and_replace(&old_str, &new_str, &(path.into())).await?;
        Ok(())
    }
}

impl CargoCheck {
    async fn cargo_check(&self) -> anyhow::Result<CargoCheckResult> {
        let res = Cargo::cargo_check().await?;
        match res {
            cargo::CargoCheck::CheckPasses { warnings } => {
                let vec = if self.input.include_warnings.unwrap_or(false) {
                    warnings
                        .into_iter()
                        .map(|x| x.message.to_string())
                        .collect()
                } else {
                    vec![]
                };
                Ok(CargoCheckResult::Success(vec))
            }
            cargo::CargoCheck::CheckFailed { failures, warnings } => {
                let vec = if self.input.include_warnings.unwrap_or(false) {
                    warnings
                        .into_iter()
                        .map(|x| x.message.to_string())
                        .collect()
                } else {
                    vec![]
                };
                let failures = failures
                    .into_iter()
                    .map(|x| x.message.to_string())
                    .collect();
                Ok(CargoCheckResult::Failed {
                    warnings: vec,
                    errors: failures,
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool_defs::{InsertAfterLine, InsertAfterLineInput};
    use std::env;

    fn temp_path(name: &str) -> String {
        env::temp_dir()
            .join(format!("turbo_code_test_{name}"))
            .to_string_lossy()
            .into_owned()
    }

    fn make_insert(file_path: String, line_num: usize, content: &str) -> InsertAfterLine {
        InsertAfterLine {
            input: InsertAfterLineInput {
                content: content.to_string(),
                file_path,
                line_num,
            },
            id: String::new(),
        }
    }

    #[tokio::test]
    async fn test_insert_after_line_middle() {
        let path = temp_path("middle");
        fs::write(&path, "aaa\nbbb\nccc\n").await.unwrap();
        let tmp = fs::read_to_string(&path).await.unwrap();
        println!("{tmp}");

        let tool = make_insert(path.clone(), 2, "xxx");
        tool.insert_after_line().await.unwrap();

        let result = fs::read_to_string(&path).await.unwrap();
        assert_eq!(result, "aaa\nxxx\nbbb\nccc\n");
        fs::remove_file(&path).await.ok();
    }

    #[tokio::test]
    async fn test_insert_after_line_beginning() {
        let path = temp_path("beginning");
        fs::write(&path, "aaa\nbbb\nccc\n").await.unwrap();

        let tool = make_insert(path.clone(), 1, "xxx");
        tool.insert_after_line().await.unwrap();

        let result = fs::read_to_string(&path).await.unwrap();
        assert_eq!(result, "xxx\naaa\nbbb\nccc\n");
        fs::remove_file(&path).await.ok();
    }

    #[tokio::test]
    async fn test_insert_after_line_end() {
        let path = temp_path("end");
        fs::write(&path, "aaa\nbbb\nccc\n").await.unwrap();

        let tool = make_insert(path.clone(), 4, "xxx");
        tool.insert_after_line().await.unwrap();

        let result = fs::read_to_string(&path).await.unwrap();
        assert_eq!(result, "aaa\nbbb\nccc\nxxx\n");
        fs::remove_file(&path).await.ok();
    }

    #[tokio::test]
    async fn test_insert_after_line_multiline_content() {
        let path = temp_path("multiline");
        fs::write(&path, "aaa\nbbb\nccc\n").await.unwrap();

        let tool = make_insert(path.clone(), 2, "xxx\nyyy\n");
        tool.insert_after_line().await.unwrap();

        let result = fs::read_to_string(&path).await.unwrap();
        assert_eq!(result, "aaa\nxxx\nyyy\nbbb\nccc\n");
        fs::remove_file(&path).await.ok();
    }
}
