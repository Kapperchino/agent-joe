use crate::tool_defs::{NonEmptyString, ToolId, ToolInvocation, ToolResult};
use serde_json::Value;

impl ToolResult {
    pub fn success(id: ToolId, invocation: ToolInvocation, content: String) -> ToolResult {
        ToolResult::Success {
            id,
            invocation,
            content,
        }
    }

    pub fn error(
        id: ToolId,
        name: NonEmptyString,
        input: serde_json::Map<String, Value>,
        message: String,
    ) -> ToolResult {
        ToolResult::Failure {
            id,
            msg: message,
            name,
            input,
        }
    }

    pub fn name(&self) -> String {
        match &self {
            ToolResult::Success { invocation, .. } => invocation.name.to_string(),
            ToolResult::Failure { name, .. } => name.to_string(),
        }
    }

    pub fn id(&self) -> ToolId {
        match self {
            ToolResult::Success { id, .. } => id.clone(),
            ToolResult::Failure { id, .. } => id.clone(),
        }
    }
}
