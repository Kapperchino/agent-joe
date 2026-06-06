use crate::tool_defs::{ToolId, ToolInvocation, ToolResult};
use serde_json::Value;

impl ToolResult {
    pub fn success(id: ToolId, invocation: ToolInvocation, content: String) -> ToolResult {
        ToolResult::Success {
            id,
            invocation,
            content,
        }
    }

    pub fn error(id: ToolId, name: String, input: Value, message: String) -> ToolResult {
        ToolResult::Failure {
            id,
            msg: message,
            name,
            input,
        }
    }

    pub fn name(&self) -> String {
        match &self {
            ToolResult::Success { invocation, .. } => invocation.name.clone(),
            ToolResult::Failure { name, .. } => name.clone(),
        }
    }

    pub fn id(&self) -> ToolId {
        match self {
            ToolResult::Success { id, .. } => id.clone(),
            ToolResult::Failure { id, .. } => id.clone(),
        }
    }
}
