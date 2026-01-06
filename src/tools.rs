use crate::claude;
use crate::claude::{ToolProperty, ToolSchemaDTO};
use crate::utils::Utils;
use anyhow::Error;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use tokio::fs;
use tokio::fs::DirEntry;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Tool {
    ReadFile(ReadFile),
}

impl ToolTrait for Tool {
    fn name(&self) -> String {
        match self {
            Tool::ReadFile(_) => "read_file".to_string(),
        }
    }

    fn description(&self) -> String {
        match self {
            Tool::ReadFile(_) => "Reads a file at file_path".to_string(),
        }
    }

    fn field_properties(&self) -> HashMap<String, ToolProperty> {
        match self {
            Tool::ReadFile(_) => HashMap::from([(
                "file_path".to_string(),
                ToolProperty {
                    name: "file_path".to_string(),
                    prop_type: "string".to_string(),
                    description: "file path of the file you want to read".to_string(),
                },
            )]),
        }
    }

    fn required_fields(&self) -> Vec<String> {
        match self {
            Tool::ReadFile(_) => {
                vec!["file_path".to_string()]
            }
        }
    }

    fn id(&self) -> String {
        match self {
            Tool::ReadFile(file) => file.id.clone(),
        }
    }
}

#[derive(Default, Serialize, Deserialize, Debug, Clone)]
pub struct ReadFile {
    pub input: ReadFileInput,
    pub id: String,
}

#[derive(Default, Serialize, Deserialize, Debug, Clone)]
pub struct ListFiles {
    pub input: ListFilesInput,
    pub id: String,
}

#[derive(Default, Serialize, Deserialize, Debug, Clone)]
pub struct ListFilesInput {
    pub(crate) dir_path: String,
}

#[derive(Default, Serialize, Deserialize, Debug, Clone)]
pub struct ReadFileInput {
    pub(crate) file_path: String,
}
#[derive(Debug)]
pub enum ToolResult {
    ReadFileResult {
        res: String,
        tool: Tool,
        id: String,
    },
    ListFilesResult {
        files: Vec<String>,
        dirs: Vec<String>,
        tool: Tool,
        id: String,
    },
}

pub trait ToolTrait {
    fn name(&self) -> String;
    fn description(&self) -> String;
    fn field_properties(&self) -> HashMap<String, ToolProperty>;
    fn required_fields(&self) -> Vec<String>;
    fn id(&self) -> String;
}

impl ToolResult {
    pub fn tool(&self) -> Tool {
        match self {
            ToolResult::ReadFileResult {
                res: _res,
                tool,
                id: _id,
            } => tool.clone(),
            ToolResult::ListFilesResult { tool, .. } => tool.clone(),
        }
    }

    pub fn to_res_json(&self) -> claude::ContentBlock {
        match self {
            ToolResult::ReadFileResult { res, tool: _, id } => claude::ContentBlock::ToolResult {
                tool_use_id: id.to_string(),
                content: res.to_string(),
            },
            ToolResult::ListFilesResult {
                files, dirs, id, ..
            } => claude::ContentBlock::ToolResult {
                tool_use_id: id.to_string(),
                content: format!("files: {:?}\ndirs: {:?}", files, dirs),
            },
        }
    }
}

impl Tool {
    pub async fn use_tool(&self, id: String) -> anyhow::Result<ToolResult> {
        match self {
            Tool::ReadFile(path) => {
                let result = fs::read_to_string(&path.input.file_path).await?;
                Ok(ToolResult::ReadFileResult {
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
            _ => Err(Error::msg("Is not a tool")),
        }
    }

    pub fn to_json(&self) -> claude::Tool {
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

    pub fn to_req(&self) -> HashMap<String, String> {
        match self {
            Tool::ReadFile(path) => {
                HashMap::from([("file_path".to_string(), path.input.file_path.to_string())])
            }
        }
    }
}
