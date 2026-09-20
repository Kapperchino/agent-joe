use super::*;
use crate::session::{Event, Operation, PendingBatch};
use clients::{
    llm::{Message, Role, SessionProvider},
    response::ToolCall,
};
use tools::tool_defs::{ToolId, ToolInvocation, ToolResult};

fn archive(session: &Session, id: &str) {
    let call = ToolCall {
        id: ToolId {
            call_id: None,
            id: id.to_owned().try_into().unwrap(),
        },
        name: "read_file".to_owned().try_into().unwrap(),
        input: Default::default(),
    };
    session
        .record(Event::Prepared(PendingBatch {
            assistant: Message {
                role: Role::Assistant,
                content: vec![call.content()],
            },
            operations: vec![Operation::new(id.into(), call)],
        }))
        .unwrap();
    session
        .record(Event::Intent {
            operation: id.into(),
            effect: tools::tool_defs::ToolEffect::Read,
        })
        .unwrap();
    session
        .complete_tool(
            id.into(),
            ToolResult {
                id: ToolId {
                    call_id: None,
                    id: id.to_owned().try_into().unwrap(),
                },
                invocation: ToolInvocation {
                    name: "read_file".to_owned().try_into().unwrap(),
                    input: Default::default(),
                    display: String::new(),
                },
                outcome: Ok("evidence\n".repeat(2000)),
            },
        )
        .unwrap();
    let messages = session.snapshot().unwrap().pending.unwrap().messages();
    session.record(Event::History(messages.into())).unwrap();
}

#[test]
fn worker_reports_exclude_inherited_artifacts_and_detach_cleanly() {
    let workspace = crate::session::tests::Workspace::new();
    let store = workspace.store();
    let parent = store
        .create(SessionProvider::Injected, None, Vec::new())
        .unwrap();
    archive(&parent, "parent-evidence");
    let child = store
        .create(
            SessionProvider::Injected,
            Some(parent.id.clone()),
            Vec::new(),
        )
        .unwrap();
    let worker = WorkerSession::default();
    worker.attach(Some(child.clone())).unwrap();
    assert!(worker.evidence().artifacts.is_empty());
    archive(&child, "child-evidence");
    let evidence = worker.evidence();
    assert_eq!(evidence.artifacts.len(), 1);
    assert_eq!(
        evidence.artifacts[0].id,
        child.snapshot().unwrap().artifacts[1].id
    );
    worker.attach(None).unwrap();
    assert!(worker.evidence().artifacts.is_empty());
    assert!(worker.evidence().unresolved_issues.is_empty());
}

#[test]
fn missing_session_evidence_is_reported_and_failed_attachment_preserves_the_previous_session() {
    let workspace = crate::session::tests::Workspace::new();
    let store = workspace.store();
    let session = store
        .create(SessionProvider::Injected, None, Vec::new())
        .unwrap();
    let worker = WorkerSession::default();
    worker.attach(Some(session.clone())).unwrap();
    archive(&session, "observed-evidence");
    let invalid = store
        .create(SessionProvider::Injected, None, Vec::new())
        .unwrap();
    crate::session::tests::invalidate(&store, &invalid.id);
    assert!(worker.attach(Some(invalid)).is_err());
    assert_eq!(worker.evidence().artifacts.len(), 1);
    crate::session::tests::invalidate(&store, &session.id);
    let evidence = worker.evidence();
    assert!(evidence.artifacts.is_empty());
    assert_eq!(evidence.unresolved_issues.len(), 1);
    assert!(evidence.unresolved_issues[0].contains("Could not retrieve worker session evidence"));
}
