use crate::claude;
use crate::claude::{ToolProperty, ToolSchemaDTO};
use anyhow::Error;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tokio::fs;
use tokio::fs::File;
use tokio::io::{AsyncBufReadExt, BufReader};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Tool {
    ReadFile(ReadFile),
}

impl ToolTrait for Tool {
    fn name(&self) -> String {
        match self {
            Tool::ReadFile(_) => "file_path".to_string(),
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
}

#[derive(Default, Serialize, Deserialize, Debug, Clone)]
pub struct ReadFile {
    pub(crate) file_path: String,
    pub id: String,
}
#[derive(Debug)]
pub enum ToolResult {
    ReadFileResult { res: String, tool: Tool, id: String },
}

pub trait ToolTrait {
    fn name(&self) -> String;
    fn description(&self) -> String;
    fn field_properties(&self) -> HashMap<String, ToolProperty>;
    fn required_fields(&self) -> Vec<String>;
}

impl ToolResult {
    pub fn tool(&self) -> Tool {
        match self {
            ToolResult::ReadFileResult {
                res: _res,
                tool,
                id: _id,
            } => tool.clone(),
        }
    }
}

impl Tool {
    pub async fn use_tool(&self, id: String) -> Result<ToolResult, anyhow::Error> {
        match self {
            Tool::ReadFile(path) => {
                let result = fs::read_to_string(&path.file_path).await?;
                Ok(ToolResult::ReadFileResult {
                    res: result,
                    tool: self.clone(),
                    id,
                })
            }
        }
    }

    pub fn from_str(name: &str) -> Result<Self, anyhow::Error> {
        match name {
            "file_path" => Ok(Tool::ReadFile(ReadFile::default())),
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
                required: vec!["file_path".into()],
            },
        }
    }

    pub fn to_req(&self) -> HashMap<String, String> {
        match self {
            Tool::ReadFile(path) => {
                HashMap::from([("file_path".to_string(), path.file_path.to_string())])
            }
        }
    }
}
