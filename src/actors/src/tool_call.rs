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
    pub fn content(&self) -> clients::llm::ContentBlock {
        clients::llm::ContentBlock::ToolBlock {
            tool_id: self.id.clone(),
            name: self.name.clone(),
            input: self.input.clone(),
        }
    }
    pub fn error_content(&self, message: &str) -> clients::llm::ContentBlock {
        clients::llm::ContentBlock::ToolResult {
            tool_id: self.id.clone(),
            content: message.into(),
            is_error: Some(true),
        }
    }
    pub fn failed(&self, failure: tools::tool_error::ToolFailure) -> tools::tool_defs::ToolResult {
        tools::tool_defs::ToolResult {
            id: self.id.clone(),
            invocation: tools::tool_defs::ToolInvocation {
                name: self.name.clone(),
                input: self.input.clone(),
                display: self.name.to_string(),
            },
            outcome: Err(failure),
        }
    }
}
