use crate::{claude, openai};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use turbo_code_macros::ToolDef;
use turbo_code_macros::ToolInput;

pub trait ToolDefTrait {
    fn tool_name() -> &'static str;
    fn tool_description() -> &'static str;
    fn field_properties() -> HashMap<String, ToolProperty>;
    fn required_fields() -> Vec<String>;
    fn req(&self) -> anyhow::Result<HashMap<String, String>>;
}

pub trait ToolInputSchema {
    fn properties() -> HashMap<String, ToolProperty>;
    fn required() -> Vec<String>;
    fn req(&self) -> anyhow::Result<HashMap<String, String>>;
}

pub trait ToolUse {}

pub trait LenientDeserialize: Sized {
    fn deserialize_lenient(s: &str) -> anyhow::Result<Self>;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ToolJson {
    OpenAI(openai::Tool),
    Claude(claude::Tool),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Tool {
    ReadFile(ReadFile),
    InsertAfterLine(InsertAfterLine),
    StringReplace(StringReplace),
    CargoCheck(CargoCheck),
}

#[derive(Debug, Clone)]
pub enum ToolProperty {
    Value {
        name: String,
        prop_type: String,
        description: String,
    },
    Object {
        name: String,
        prop_type: String,
        description: String,
        properties: HashMap<String, ToolProperty>,
    },
}

#[derive(Serialize, Deserialize, Debug, Clone, Default, Eq, PartialEq, Hash, ToolInput)]
pub struct Range {
    #[tool(description = "Start line (inclusive)", required)]
    pub start: u32,
    #[tool(description = "End line (exclusive)", required)]
    pub end: u32,
}

#[derive(Default, Serialize, Deserialize, Debug, Clone, ToolDef)]
#[tool(
    name = "read_file",
    description = "Reads a file at file_path or a section of it defined by range or the files in a director if a dir path is provided,\
     do not read the entire file unless you need to"
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
    #[tool(description = "range of the offsets you want to read, empty to read the entire file")]
    pub range: Option<Range>,
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

#[derive(Default, Serialize, Deserialize, Debug, Clone, ToolDef)]
#[tool(
    name = "cargo_check",
    description = "Run cargo check on the project to find compilation errors and warnings"
)]
pub struct CargoCheck {
    #[tool(input)]
    pub input: CargoCheckInput,
    pub id: String,
}

#[derive(Default, Serialize, Deserialize, Debug, Clone, ToolInput)]
pub struct CargoCheckInput {
    #[serde(default)]
    #[tool(description = "Include warnings in the output, defaults to false")]
    pub include_warnings: Option<bool>,
}

#[derive(Debug)]
pub enum CargoCheckResult {
    Success(Vec<String>),
    Failed {
        warnings: Vec<String>,
        errors: Vec<String>,
    },
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
    StringReplaceResult {
        status: String,
        tool: Tool,
        id: String,
    },
    CargoCheckResult {
        status: String,
        result: CargoCheckResult,
        tool: Tool,
        id: String,
    },
    Error {
        message: String,
        tool: Tool,
        id: String,
    },
}
