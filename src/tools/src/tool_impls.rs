use crate::tool_defs::{ToolId, ToolInvocation, ToolResult};

impl ToolResult {
    pub fn success(id: ToolId, invocation: ToolInvocation, content: String) -> ToolResult {
        ToolResult {
            id,
            invocation,
            content,
            is_error: false,
        }
    }

    pub fn error(id: ToolId, invocation: ToolInvocation, message: String) -> ToolResult {
        ToolResult {
            id,
            invocation,
            content: message,
            is_error: true,
        }
    }

    pub fn id(&self) -> ToolId {
        self.id.clone()
    }
}
