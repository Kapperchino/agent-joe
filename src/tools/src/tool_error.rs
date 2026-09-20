use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum ToolFailureKind {
    InvalidInput,
    Execution,
    Validation,
    Worker,
    Timeout,
    Cancelled,
    Panicked,
    Persistence,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum FailureImpact {
    NotStarted,
    NoWorkspaceChange,
    MayHaveChanged,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ToolFailure {
    pub kind: ToolFailureKind,
    pub impact: FailureImpact,
    pub message: String,
}
impl ToolFailure {
    pub fn new(kind: ToolFailureKind, impact: FailureImpact, message: impl Into<String>) -> Self {
        Self {
            kind,
            impact,
            message: message.into(),
        }
    }

    pub fn stops_turn(&self) -> bool {
        self.impact == FailureImpact::MayHaveChanged
            || matches!(
                self.kind,
                ToolFailureKind::Worker
                    | ToolFailureKind::Timeout
                    | ToolFailureKind::Cancelled
                    | ToolFailureKind::Panicked
                    | ToolFailureKind::Persistence
            )
    }
}
impl fmt::Display for ToolFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let effects = match self.impact {
            FailureImpact::MayHaveChanged => {
                ". Effects may be partial; inspect the workspace before retrying"
            }
            _ => "",
        };
        write!(f, "{:?}: {}{effects}", self.kind, self.message)
    }
}
impl std::error::Error for ToolFailure {}

#[cfg(test)]
#[path = "../tests/unit/tool_error/tests.rs"]
mod tests;
