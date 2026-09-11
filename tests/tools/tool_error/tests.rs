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
