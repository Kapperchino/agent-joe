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
    ListFiles(ListFiles),
}

impl ToolTrait for Tool {
    fn name(&self) -> String {
        match self {
            Tool::ReadFile(_) => "read_file".to_string(),
            Tool::ListFiles(_) => "read_dir".to_string(),
        }
    }

    fn description(&self) -> String {
        match self {
            Tool::ReadFile(_) => "Reads a file at file_path".to_string(),
            Tool::ListFiles(_) => "Gets the list of files at dir".to_string(),
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
            Tool::ListFiles(_) => HashMap::from([(
                "dir_path".to_string(),
                ToolProperty {
                    name: "dir_path".to_string(),
                    prop_type: "string".to_string(),
                    description: "directory path of the directory you want to read".to_string(),
                },
            )]),
        }
    }

    fn required_fields(&self) -> Vec<String> {
        match self {
            Tool::ReadFile(_) => {
                vec!["file_path".to_string()]
            }
            Tool::ListFiles(_) => {
                vec!["dir_path".to_string()]
            }
        }
    }

    fn id(&self) -> String {
        match self {
            Tool::ReadFile(file) => file.id.clone(),
            Tool::ListFiles(files) => files.id.clone(),
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
            Tool::ListFiles(path) => {
                let entires =
                    Utils::get_dir_files(&PathBuf::from(path.input.dir_path.clone())).await?;

                let mut dirs: Vec<String> = vec![];
                let mut files: Vec<String> = vec![];

                fn get_path(entry: &DirEntry) -> Option<String> {
                    entry.path().to_str().map(|str| str.to_string())
                }

                for entry in entires {
                    match entry.file_type().await {
                        Ok(ftype) => {
                            if ftype.is_file()
                                && let Some(fpath) = get_path(&entry)
                            {
                                files.push(fpath);
                            } else if ftype.is_dir()
                                && let Some(fpath) = get_path(&entry)
                            {
                                dirs.push(fpath);
                            }
                        }
                        Err(_) => {}
                    }
                }

                Ok(ToolResult::ListFilesResult {
                    files,
                    dirs,
                    tool: self.clone(),
                    id,
                })
            }
        }
    }

    pub fn from_str(name: &str) -> anyhow::Result<Self> {
        match name {
            "read_file" => Ok(Tool::ReadFile(ReadFile::default())),
            "read_dir" => Ok(Tool::ListFiles(ListFiles::default())),
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
            Tool::ListFiles(path) => {
                HashMap::from([("dir_path".to_string(), path.input.dir_path.to_string())])
            }
        }
    }
}
