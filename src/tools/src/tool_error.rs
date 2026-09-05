use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ToolFailureKind {
    InvalidInput,
    Execution,
    Validation,
    Worker,
    Timeout,
    Cancelled,
    Panicked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolEffects {
    NotStarted,
    NoWorkspaceChange,
    MayHaveChanged,
}

#[derive(Debug, Clone)]
pub struct ToolFailure {
    pub kind: ToolFailureKind,
    pub effects: ToolEffects,
    pub message: String,
}
impl ToolFailure {
    pub fn new(kind: ToolFailureKind, effects: ToolEffects, message: impl Into<String>) -> Self {
        Self {
            kind,
            effects,
            message: message.into(),
        }
    }

    pub fn stops_turn(&self) -> bool {
        self.effects == ToolEffects::MayHaveChanged
            || matches!(
                self.kind,
                ToolFailureKind::Worker
                    | ToolFailureKind::Timeout
                    | ToolFailureKind::Cancelled
                    | ToolFailureKind::Panicked
            )
    }
}
impl fmt::Display for ToolFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let effects = match self.effects {
            ToolEffects::MayHaveChanged => {
                ". Effects may be partial; inspect the workspace before retrying"
            }
            _ => "",
        };
        write!(f, "{:?}: {}{effects}", self.kind, self.message)
    }
}
impl std::error::Error for ToolFailure {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostics_do_not_determine_whether_a_tool_stops_the_turn() {
        let misleading = "timeout, panicked worker, partial effects";
        assert!(
            !ToolFailure::new(
                ToolFailureKind::Execution,
                ToolEffects::NoWorkspaceChange,
                misleading
            )
            .stops_turn()
        );
        assert!(
            ToolFailure::new(
                ToolFailureKind::Timeout,
                ToolEffects::NoWorkspaceChange,
                "plain diagnostic"
            )
            .stops_turn()
        );
        assert!(
            ToolFailure::new(
                ToolFailureKind::Execution,
                ToolEffects::MayHaveChanged,
                "plain diagnostic"
            )
            .stops_turn()
        );
    }
}
