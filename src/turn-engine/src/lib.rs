pub mod machine;
pub mod turn;
use common_models::runtime_ids::{OperationId, WorkspaceRevision};
use tools::{
    tool_defs::{ToolOpKind, ToolResult},
    tool_error::ToolFailure,
};

#[derive(Debug)]
pub enum ToolEvent {
    Started {
        operation: OperationId,
        effect: ToolOpKind,
        revision: Option<WorkspaceRevision>,
        display: String,
    },
    Completed {
        operation: OperationId,
        result: ToolResult,
    },
    Finished(Result<(), ToolFailure>),
}

#[derive(Debug, thiserror::Error)]
pub enum WorkerFailure {
    #[error("Worker is already running")]
    AlreadyRunning,
    #[error("Worker startup failed: {0}")]
    Startup(String),
    #[error("Worker could not start its turn: {0}")]
    Mailbox(String),
    #[error("Worker failed: {0}")]
    Turn(clients::failure::Failure),
    #[error("Worker cancelled")]
    Cancelled,
    #[error("Worker stopped without a result")]
    Stopped,
    #[error("Worker task terminated: {0}")]
    Join(String),
}
impl WorkerFailure {
    pub fn into_tool_failure(self) -> tools::tool_error::ToolFailure {
        use tools::tool_error::{FailureImpact, ToolFailure, ToolFailureKind};
        let effects = match self {
            Self::Startup(_) | Self::AlreadyRunning => FailureImpact::NotStarted,
            _ => FailureImpact::MayHaveChanged,
        };
        ToolFailure::new(ToolFailureKind::Worker, effects, self.to_string())
    }
}
