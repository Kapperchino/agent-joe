use crate::analysis::Range;
use crate::claude;
use crate::claude::ToolProperty;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub trait ToolTrait {
    fn name(&self) -> String;
    fn description(&self) -> String;
    fn field_properties(&self) -> HashMap<String, ToolProperty>;
    fn required_fields(&self) -> Vec<String>;
    fn id(&self) -> String;
    fn to_json(&self) -> claude::Tool;
    fn to_req(&self) -> HashMap<String, String>;
}

pub trait ToolResultTrait {
    fn tool(&self) -> Tool;
    fn to_res_json(&self) -> claude::ContentBlock;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Tool {
    ReadFile(ReadFile),
    InsertAfterLine(InsertAfterLine),
}

#[derive(Default, Serialize, Deserialize, Debug, Clone)]
pub struct ReadFile {
    pub input: ReadFileInput,
    pub id: String,
}
#[derive(Default, Serialize, Deserialize, Debug, Clone)]
pub struct ReadFileInput {
    pub(crate) file_path: String,
    pub range: Option<Range>,
}

#[derive(Default, Serialize, Deserialize, Debug, Clone)]
pub struct InsertAfterLine {
    pub input: InsertAfterLineInput,
    pub id: String,
}
#[derive(Default, Serialize, Deserialize, Debug, Clone)]
pub struct InsertAfterLineInput {
    pub content: String,
    pub(crate) file_path: String,
    pub line_num: usize,
}

#[derive(Debug)]
pub enum ToolResult {
    ReadFileResult {
        res: String,
        tool: Tool,
        id: String,
    },
    InsertAfterLineResult {
        status: String,
        tool: Tool,
        id: String,
    },
}
