use super::*;
use tools::{tool_defs::ToolId, tool_error::FailureImpact};

fn call(id: &str) -> ToolCall {
    ToolCall {
        id: ToolId {
            id: id.to_owned().try_into().unwrap(),
            call_id: None,
        },
        name: "read".to_owned().try_into().unwrap(),
        input: Default::default(),
    }
}

fn failure(call: &ToolCall) -> ToolResult {
    call.failed(ToolFailure::new(
        ToolFailureKind::Execution,
        FailureImpact::NoWorkspaceChange,
        "file unavailable",
    ))
}

#[test]
fn batch_rejects_unknown_mismatched_and_duplicate_completions() {
    let call = call("accepted");
    let mut batch = ToolBatch::new(TurnId::new(), vec![ProcessedItem::Tool(call.clone())]);
    let operation = batch.jobs()[0].operation;
    assert!(batch.complete(OperationId::new(), failure(&call)).is_none());
    let mut other = call.clone();
    other.id.id = "other".to_owned().try_into().unwrap();
    assert!(batch.complete(operation, failure(&other)).is_none());
    assert!(batch.start(operation).is_some());
    assert!(batch.start(operation).is_none());
    assert!(batch.complete(operation, failure(&call)).is_some());
    assert!(batch.complete(operation, failure(&call)).is_none());
    assert!(batch.start(operation).is_none());
    assert_eq!(batch.pending_operations().count(), 0);
    assert!(
        matches!(&batch.messages()[1].content[0], ContentBlock::ToolResult {
            tool_id, content, is_error: Some(true),
        } if *tool_id == call.id && content == "Execution: file unavailable")
    );
}

#[test]
fn workspace_revision_resets_the_repeated_failure_budget() {
    let revision = WorkspaceRevision(0);
    let mut failures = FailureTracker::default();
    let result = failure(&call("accepted"));
    for _ in 0..2 {
        assert!(matches!(
            failures.record(&result, revision),
            Continuation::Continue
        ));
    }
    let revision = WorkspaceRevision(1);
    for _ in 0..2 {
        assert!(matches!(
            failures.record(&result, revision),
            Continuation::Continue
        ));
    }
    assert!(matches!(
        failures.record(&result, revision),
        Continuation::Stop(_)
    ));
}
