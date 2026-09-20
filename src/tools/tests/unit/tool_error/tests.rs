use super::*;

#[test]
fn diagnostics_do_not_determine_whether_a_tool_stops_the_turn() {
    let misleading = "timeout, panicked worker, partial effects";
    assert!(
        !ToolFailure::new(
            ToolFailureKind::Execution,
            FailureImpact::NoWorkspaceChange,
            misleading
        )
        .stops_turn()
    );
    assert!(
        ToolFailure::new(
            ToolFailureKind::Timeout,
            FailureImpact::NoWorkspaceChange,
            "plain diagnostic"
        )
        .stops_turn()
    );
    assert!(
        ToolFailure::new(
            ToolFailureKind::Execution,
            FailureImpact::MayHaveChanged,
            "plain diagnostic"
        )
        .stops_turn()
    );
}
