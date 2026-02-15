use crate::analysis::Range;
use crate::claude;
use crate::claude::{ToolProperty, ToolSchemaDTO};
use crate::cur_context::CurContext;
pub(crate) use crate::tool_defs::{InsertAfterLine, ToolResultTrait};
pub(crate) use crate::tool_defs::{ReadFile, Tool, ToolResult, ToolTrait};
use anyhow::{anyhow, Error};
use futures::{StreamExt, TryStreamExt};
use itertools::Itertools;
use ra_ap_ide::TextSize;
use std::collections::HashMap;
use std::io::SeekFrom;
use tokio::fs;
use tokio::fs::File;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncSeekExt, BufReader};
use tokio_stream::wrappers::LinesStream;

impl ToolTrait for Tool {
    fn name(&self) -> String {
        match self {
            Tool::ReadFile(_) => "read_file".to_string(),
            Tool::InsertAfterLine(_) => "insert_after_line".to_string(),
        }
    }

    fn description(&self) -> String {
        match self {
            Tool::ReadFile(_) => "Reads a file at file_path or a section of it defined by range, do not read the entire file unless you need to".to_string(),
            Tool::InsertAfterLine(_) => "Insert content at line_num".to_string(),
        }
    }

    fn field_properties(&self) -> HashMap<String, ToolProperty> {
        match self {
            Tool::ReadFile(_) => HashMap::from([
                (
                    "file_path".to_string(),
                    ToolProperty::Value {
                        name: "file_path".to_string(),
                        prop_type: "string".to_string(),
                        description: "file path of the file you want to read".to_string(),
                    },
                ),
                (
                    "range".to_string(),
                    ToolProperty::Object {
                        name: "range".to_string(),
                        prop_type: "string".to_string(),
                        description: "range of the offsets you want to read".to_string(),
                        properties: HashMap::from([
                            (
                                "start".to_string(),
                                ToolProperty::Value {
                                    name: "start".to_string(),
                                    prop_type: "integer".to_string(),
                                    description: "Start line (inclusive)".to_string(),
                                },
                            ),
                            (
                                "end".to_string(),
                                ToolProperty::Value {
                                    name: "end".to_string(),
                                    prop_type: "integer".to_string(),
                                    description: "End line (exclusive)".to_string(),
                                },
                            ),
                        ]),
                    },
                ),
            ]),
            Tool::InsertAfterLine(_) => HashMap::from([
                (
                    "content".to_string(),
                    ToolProperty::Value {
                        name: "content".to_string(),
                        prop_type: "string".to_string(),
                        description: "Content to insert after line line_num".to_string(),
                    },
                ),
                (
                    "file_path".to_string(),
                    ToolProperty::Value {
                        name: "file_path".to_string(),
                        prop_type: "string".to_string(),
                        description: "Path of the file to insert".to_string(),
                    },
                ),
                (
                    "line_num".to_string(),
                    ToolProperty::Value {
                        name: "file_path".to_string(),
                        prop_type: "number".to_string(),
                        description: "Line number of the file to insert to".to_string(),
                    },
                ),
            ]),
        }
    }

    fn required_fields(&self) -> Vec<String> {
        match self {
            Tool::ReadFile(_) => {
                vec!["file_path".to_string()]
            }
            Tool::InsertAfterLine(_) => {
                vec![
                    "content".to_string(),
                    "file_path".to_string(),
                    "line_num".to_string(),
                ]
            }
        }
    }

    fn id(&self) -> String {
        match self {
            Tool::ReadFile(file) => file.id.clone(),
            Tool::InsertAfterLine(insert) => insert.id.clone(),
        }
    }

    fn to_json(&self) -> claude::Tool {
        claude::Tool {
            name: self.name(),
            description: self.description(),
            input_schema: ToolSchemaDTO {
                name: self.name(),
                tool_type: "object".to_string(),
                properties: self.field_properties(),
                required: self.required_fields(),
            },
        }
    }

    fn to_req(&self) -> HashMap<String, String> {
        match self {
            Tool::ReadFile(path) => {
                HashMap::from([("file_path".to_string(), path.input.file_path.to_string())])
            }
            Tool::InsertAfterLine(insert) => HashMap::from([
                ("content".to_string(), insert.input.content.to_string()),
                ("file_path".to_string(), insert.input.file_path.to_string()),
                ("line_num".to_string(), insert.input.line_num.to_string()),
            ]),
        }
    }
}

impl ToolResultTrait for ToolResult {
    fn tool(&self) -> Tool {
        match self {
            ToolResult::ReadFileResult {
                res: _res,
                tool,
                id: _id,
            } => tool.clone(),
            ToolResult::InsertAfterLineResult { status, tool, id } => tool.clone(),
        }
    }

    fn to_res_json(&self) -> claude::ContentBlock {
        match self {
            ToolResult::ReadFileResult { res, tool: _, id } => claude::ContentBlock::ToolResult {
                tool_use_id: id.to_string(),
                content: res.to_string(),
            },
            ToolResult::InsertAfterLineResult { status, tool, id } => {
                claude::ContentBlock::ToolResult {
                    tool_use_id: id.to_string(),
                    content: status.to_string(),
                }
            }
        }
    }
}

// these can't be traits
impl Tool {
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
        }
    }

    pub fn from_str(name: &str) -> anyhow::Result<Self> {
        match name {
            "read_file" => Ok(Tool::ReadFile(ReadFile::default())),
            "insert_after_line" => Ok(Tool::InsertAfterLine(InsertAfterLine::default())),
            _ => Err(Error::msg("Is not a tool")),
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
                let mut buf = vec![0; (end - start) as usize];
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
