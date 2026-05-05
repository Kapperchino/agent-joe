use crate::claude;
use crate::claude::ToolSchemaDTO;
use crate::llm::ContentBlock;
use crate::tool_defs::GrepTool;
use crate::tool_defs::InsertAfterLine;
use crate::tool_defs::StringReplace;
use crate::tool_defs::ToolDefTrait;
use crate::tool_defs::{CargoCheck, CargoTest, ToolId};
use crate::tool_defs::{CargoCheckResult, CargoTestResult, Tool};
use crate::tool_defs::{Range, ReadFile, StringReplaceInput, ToolJson, ToolResult};
use analysis::contexts::context::{Context, LineIndexCreator};
use anyhow::Error;
use futures::{StreamExt, TryStreamExt};
use std::cmp::min;
use std::collections::HashMap;
use std::fmt::{Display, Formatter};
use std::io::SeekFrom;
use std::path::PathBuf;
use tokio::fs;
use tokio::fs::File;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncSeekExt, BufReader};
use tokio_stream::wrappers::LinesStream;
use utils::cargo;
use utils::cargo::Cargo;
use utils::grep::Grep as ProjectGrep;
use utils::text_search::TextSearch;

impl Tool {
    pub fn name(&self) -> String {
        match self {
            Tool::ReadFile(_) => ReadFile::tool_name().to_string(),
            Tool::InsertAfterLine(_) => InsertAfterLine::tool_name().to_string(),
            Tool::StringReplace(_) => StringReplace::tool_name().to_string(),
            Tool::CargoCheck(_) => CargoCheck::tool_name().to_string(),
            Tool::Grep(_) => GrepTool::tool_name().to_string(),
            Tool::CargoTest(_) => CargoTest::tool_name().to_string(),
        }
    }

    pub fn id(&self) -> String {
        match self {
            Tool::ReadFile(file) => file.id.clone(),
            Tool::InsertAfterLine(insert) => insert.id.clone(),
            Tool::StringReplace(replace) => replace.id.clone(),
            Tool::CargoCheck(check) => check.id.clone(),
            Tool::Grep(grep) => grep.id.clone(),
            Tool::CargoTest(test) => test.id.clone(),
        }
    }

    pub fn to_json(&self) -> ToolJson {
        ToolJson::Claude(claude::Tool {
            name: self.name(),
            description: match self {
                Tool::ReadFile(_) => ReadFile::tool_description().to_string(),
                Tool::InsertAfterLine(_) => InsertAfterLine::tool_description().to_string(),
                Tool::StringReplace(_) => StringReplace::tool_description().to_string(),
                Tool::CargoCheck(_) => CargoCheck::tool_description().to_string(),
                Tool::Grep(_) => GrepTool::tool_description().to_string(),
                Tool::CargoTest(_) => CargoTest::tool_description().to_string(),
            },
            input_schema: ToolSchemaDTO {
                name: self.name(),
                tool_type: "object".to_string(),
                properties: match self {
                    Tool::ReadFile(_) => ReadFile::field_properties()
                        .into_iter()
                        .map(|(k, v)| (k, v.into()))
                        .collect(),
                    Tool::InsertAfterLine(_) => InsertAfterLine::field_properties()
                        .into_iter()
                        .map(|(k, v)| (k, v.into()))
                        .collect(),
                    Tool::StringReplace(_) => StringReplace::field_properties()
                        .into_iter()
                        .map(|(k, v)| (k, v.into()))
                        .collect(),
                    Tool::CargoCheck(_) => CargoCheck::field_properties()
                        .into_iter()
                        .map(|(k, v)| (k, v.into()))
                        .collect(),
                    Tool::Grep(_) => GrepTool::field_properties()
                        .into_iter()
                        .map(|(k, v)| (k, v.into()))
                        .collect(),
                    Tool::CargoTest(_) => CargoTest::field_properties()
                        .into_iter()
                        .map(|(k, v)| (k, v.into()))
                        .collect(),
                },
                required: match self {
                    Tool::ReadFile(_) => ReadFile::required_fields(),
                    Tool::InsertAfterLine(_) => InsertAfterLine::required_fields(),
                    Tool::StringReplace(_) => StringReplace::required_fields(),
                    Tool::CargoCheck(_) => CargoCheck::required_fields(),
                    Tool::Grep(_) => GrepTool::required_fields(),
                    Tool::CargoTest(_) => CargoTest::required_fields(),
                },
            },
        })
    }

    pub fn to_req(&self) -> anyhow::Result<HashMap<String, String>> {
        match self {
            Tool::ReadFile(path) => path.req(),
            Tool::InsertAfterLine(insert) => insert.req(),
            Tool::StringReplace(replace) => replace.req(),
            Tool::CargoCheck(check) => check.req(),
            Tool::Grep(grep) => grep.req(),
            Tool::CargoTest(test) => test.req(),
        }
    }

    pub async fn use_tool<C: Context>(&self, id: ToolId, ctx: &C) -> anyhow::Result<ToolResult> {
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
            Tool::CargoTest(test) => {
                let res = test.cargo_test().await?;
                Ok(ToolResult::CargoTestResult {
                    status: match res {
                        CargoTestResult::Success { .. } => "success",
                        CargoTestResult::Failed { .. } => "failed",
                    }
                    .to_string(),
                    result: res,
                    tool: self.clone(),
                    id,
                })
            }
            Tool::Grep(grep) => {
                let result = grep.grep(ctx).await?;
                Ok(ToolResult::GrepResult {
                    res: result,
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
            "grep" => Ok(Tool::Grep(GrepTool::default())),
            "cargo_test" => Ok(Tool::CargoTest(CargoTest::default())),
            _ => Err(Error::msg("Is not a tool")),
        }
    }
}

impl Display for Tool {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Tool::ReadFile(rf) => match &rf.input.range {
                Some(range) => write!(
                    f,
                    "- read `{}` (lines {}-{})",
                    rf.input.file_path,
                    range.start,
                    range.end.saturating_sub(1)
                ),
                None => write!(f, "- read `{}`", rf.input.file_path),
            },
            Tool::InsertAfterLine(insert) => {
                write!(
                    f,
                    "- edit `{}` after line {}",
                    insert.input.file_path, insert.input.line_num
                )
            }
            Tool::StringReplace(replace) => {
                write!(f, "- replace text in `{}`", replace.input.path)
            }
            Tool::CargoCheck(check) => {
                if check.input.include_warnings.unwrap_or(false) {
                    write!(f, "- run `cargo check` (with warnings)")
                } else {
                    write!(f, "- run `cargo check`")
                }
            }
            Tool::Grep(grep) => write!(
                f,
                "- grep `{}` (before: {}, after: {})",
                grep.input.regex, grep.input.add_start, grep.input.add_end
            ),
            Tool::CargoTest(test) => {
                if let Some(test_name) = test
                    .input
                    .test_name
                    .as_ref()
                    .filter(|name| !name.trim().is_empty())
                {
                    write!(f, "- run `cargo test {}`", test_name)
                } else {
                    write!(f, "- run `cargo test`")
                }
            }
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
            ToolResult::CargoTestResult { tool, .. } => tool.clone(),
            ToolResult::GrepResult { tool, .. } => tool.clone(),
            ToolResult::Error { tool, .. } => tool.clone(),
        }
    }

    pub fn id(&self) -> ToolId {
        match self {
            ToolResult::ReadFileResult { id, .. } => id.clone(),
            ToolResult::InsertAfterLineResult { id, .. } => id.clone(),
            ToolResult::StringReplaceResult { id, .. } => id.clone(),
            ToolResult::CargoCheckResult { id, .. } => id.clone(),
            ToolResult::CargoTestResult { id, .. } => id.clone(),
            ToolResult::GrepResult { id, .. } => id.clone(),
            ToolResult::Error { id, .. } => id.clone(),
        }
    }

    pub fn to_res_json(&self) -> ContentBlock {
        match self {
            ToolResult::ReadFileResult { res, id, .. } => ContentBlock::ToolResult {
                tool_id: id.clone(),
                content: res.to_string(),
                is_error: None,
            },
            ToolResult::InsertAfterLineResult { status, id, .. } => ContentBlock::ToolResult {
                tool_id: id.clone(),
                content: status.to_string(),
                is_error: None,
            },
            ToolResult::StringReplaceResult { status, id, .. } => ContentBlock::ToolResult {
                tool_id: id.clone(),
                content: status.to_string(),
                is_error: None,
            },
            ToolResult::CargoTestResult {
                status, result, id, ..
            } => match result {
                CargoTestResult::Success { output } => ContentBlock::ToolResult {
                    tool_id: id.clone(),
                    content: if output.trim().is_empty() {
                        status.clone()
                    } else {
                        format!("{}\n{}", status, output)
                    },
                    is_error: None,
                },
                CargoTestResult::Failed { output } => ContentBlock::ToolResult {
                    tool_id: id.clone(),
                    content: if output.trim().is_empty() {
                        status.clone()
                    } else {
                        format!("{}\n{}", status, output)
                    },
                    is_error: None,
                },
            },
            ToolResult::GrepResult { res, id, .. } => ContentBlock::ToolResult {
                tool_id: id.clone(),
                content: res.clone(),
                is_error: None,
            },
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
                        ContentBlock::ToolResult {
                            tool_id: id.clone(),
                            content: format!("{}\nWarnings:\n{}", status, warnings.join("\n")),
                            is_error: None,
                        }
                    } else {
                        ContentBlock::ToolResult {
                            tool_id: id.clone(),
                            content: status.clone(),
                            is_error: None,
                        }
                    }
                }
                CargoCheckResult::Failed { warnings, errors } => {
                    if let Tool::CargoCheck(cargo) = tool
                        && cargo.input.include_warnings.unwrap_or(false)
                    {
                        ContentBlock::ToolResult {
                            tool_id: id.clone(),
                            content: format!(
                                "{}\nWarnings:\n{}\n{}",
                                status,
                                warnings.join("\n"),
                                errors.join("\n")
                            ),
                            is_error: None,
                        }
                    } else {
                        ContentBlock::ToolResult {
                            tool_id: id.clone(),
                            content: format!("{}\nErrors:\n{}", status, warnings.join("\n")),
                            is_error: None,
                        }
                    }
                }
            },
            ToolResult::Error { message, id, .. } => ContentBlock::ToolResult {
                tool_id: id.clone(),
                content: message.clone(),
                is_error: Some(true),
            },
        }
    }
}

impl ReadFile {
    pub async fn read_file<C: Context>(&self, cur_context: &C) -> anyhow::Result<String> {
        let file_path: PathBuf = self.input.file_path.clone().into();
        let root = cur_context.get_root();
        let file_path = if file_path.starts_with(&root) {
            file_path.strip_prefix(root).map(|t| t.to_path_buf())
        } else {
            Ok(file_path)
        }?;
        match file_path.is_dir() {
            false => match &self.input.range {
                None => Self::read_entire_file(&file_path).await,
                Some(range) => Self::read_range(&file_path, range.clone(), cur_context).await,
            },
            true => Self::read_dir(&file_path).await,
        }
    }

    // one day we will have good async streams
    async fn read_dir(file_path: &PathBuf) -> anyhow::Result<String> {
        let mut entries = tokio::fs::read_dir(file_path).await?;
        let mut result = String::new();
        while let Some(entry) = entries.next_entry().await? {
            result.push_str(&entry.file_name().to_string_lossy());
            result.push('\n');
        }
        Ok(result)
    }

    async fn read_entire_file(file_path: &PathBuf) -> anyhow::Result<String> {
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
    async fn read_range<C: Context>(
        file_path: &PathBuf,
        range: Range,
        cur_context: &C,
    ) -> anyhow::Result<String> {
        let line_index = cur_context.line_index_creator().await?;
        let line_index = line_index.create_index(file_path)?;

        let start: u32 = line_index
            .line(range.start.saturating_sub(1))
            .unwrap()
            .start()
            .into();
        let end: u32 = line_index
            .line(range.end.saturating_sub(1))
            .map(|l| l.end().into())
            .unwrap_or(u32::MAX);
        let start_line = range.start.saturating_sub(1);
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
}

impl InsertAfterLine {
    async fn insert_after_line(&self) -> anyhow::Result<()> {
        // line number starts at 1 for the agent
        let line_num = self.input.line_num.clone() - 1;
        let path = self.input.file_path.clone();
        let insert_lines: Vec<_> = self.input.content.lines().collect();
        let file_content = fs::read_to_string(&path).await?;
        let mut lines: Vec<_> = file_content.lines().collect();
        let line_num = min(line_num, lines.len() - 1);
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

impl CargoTest {
    async fn cargo_test(&self) -> anyhow::Result<CargoTestResult> {
        let res = Cargo::cargo_test(
            self.input.package.as_deref(),
            self.input.test_name.as_deref(),
        )
        .await?;
        match res {
            cargo::CargoTest::TestPasses { output } => Ok(CargoTestResult::Success { output }),
            cargo::CargoTest::TestFailed { output } => Ok(CargoTestResult::Failed { output }),
        }
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
        fs::write(&path, "aaa\nbbb\nccfc\n").await.unwrap();
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
