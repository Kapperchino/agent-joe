use serde_json::{Map, Value};
use tools::tool_defs::{NonEmptyString, ToolId};

#[derive(Debug, Clone)]
pub struct ToolCall {
    pub id: ToolId,
    pub name: NonEmptyString,
    pub input: Map<String, Value>,
}

impl ToolCall {
    pub fn input_value(&self) -> Value {
        Value::Object(self.input.clone())
    }
}
