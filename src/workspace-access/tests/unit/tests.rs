use super::*;
use std::{
    future::{Future, poll_fn},
    task::Poll,
};

#[tokio::test]
async fn read_limits_and_writes_coordinate_access_and_revisions() {
    let workspace = Workspace::new(1);
    let scope = ExecutionScope::default();
    let first = workspace.acquire(ToolEffect::Read, &scope).await.unwrap();
    let second = workspace.acquire(ToolEffect::Read, &scope);
    tokio::pin!(second);
    assert!(
        poll_fn(|cx| Poll::Ready(second.as_mut().poll(cx)))
            .await
            .is_pending()
    );
    drop(first);
    let second = second.await.unwrap();
    assert_eq!(second.revision(), Some(WorkspaceRevision(0)));
    let write = workspace.acquire(ToolEffect::Write, &scope);
    tokio::pin!(write);
    assert!(
        poll_fn(|cx| Poll::Ready(write.as_mut().poll(cx)))
            .await
            .is_pending()
    );
    drop(second);
    let write = write.await.unwrap();
    assert_eq!(write.revision(), Some(WorkspaceRevision(0)));
    drop(write);
    let validation = workspace
        .acquire(ToolEffect::Validate, &scope)
        .await
        .unwrap();
    assert_eq!(validation.revision(), Some(WorkspaceRevision(1)));
    drop(validation);
    let read = workspace.acquire(ToolEffect::Read, &scope).await.unwrap();
    assert_eq!(read.revision(), Some(WorkspaceRevision(1)));
}

#[tokio::test]
async fn cancelled_waiters_release_their_read_slots() {
    let workspace = Workspace::new(1);
    let scope = ExecutionScope::default();
    let write = workspace.acquire(ToolEffect::Write, &scope).await.unwrap();
    let child = scope.child();
    let read = workspace.acquire(ToolEffect::Read, &child);
    tokio::pin!(read);
    assert!(
        poll_fn(|cx| Poll::Ready(read.as_mut().poll(cx)))
            .await
            .is_pending()
    );
    child.cancel.cancel();
    let failure = read.await.err().unwrap();
    assert_eq!(failure.kind, ToolFailureKind::Cancelled);
    drop(write);
    let read = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        workspace.acquire(ToolEffect::Read, &scope),
    )
    .await
    .unwrap()
    .unwrap();
    drop(read);
    let revision = workspace.revision();
    assert!(workspace.acquire(ToolEffect::Write, &child).await.is_err());
    assert_eq!(workspace.revision(), revision);
    assert!(workspace.is_idle());
}

#[tokio::test]
async fn managed_processes_block_edits_and_validation_until_cleanup() {
    let workspace = Workspace::new(4);
    let scope = ExecutionScope::default();
    let process = scope.register(
        utils::execution::ResourceKind::Process,
        "managed process".into(),
    );
    for effect in [ToolEffect::Read, ToolEffect::ProcessControl] {
        assert!(workspace.acquire(effect, &scope).await.is_ok());
    }
    for effect in [ToolEffect::Write, ToolEffect::Validate] {
        let failure = workspace.acquire(effect, &scope).await.err().unwrap();
        assert_eq!(failure.kind, ToolFailureKind::Validation);
        assert_eq!(failure.effects, ToolEffects::NotStarted);
        assert!(workspace.is_idle());
    }
    drop(process);
    assert!(workspace.acquire(ToolEffect::Write, &scope).await.is_ok());
    assert!(
        workspace
            .acquire(ToolEffect::Validate, &scope)
            .await
            .is_ok()
    );
}
