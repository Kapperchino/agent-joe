use super::*;
use crate::{Event, SessionStore, test_support::Workspace, tests::history};
use clients::llm::SessionProvider;
use utils::workspace::WorkspacePolicy;

#[test]
fn workspace_metadata_tracks_committed_worktrees_without_reading_full_snapshots() {
    let workspace = Workspace::new();
    let store = workspace.store();
    let session = store
        .create(SessionProvider::Injected, None, history())
        .unwrap();
    let worktree: SessionWorktree = serde_json::from_value(serde_json::json!({
        "id": "metadata-fixture",
        "path": workspace.path.join("worktree"),
        "target": "main",
    }))
    .unwrap();
    session
        .record(Event::WorktreeSource(worktree.clone()))
        .unwrap();
    let saved = session.workspace_snapshot().unwrap();
    assert!(saved.worktree.is_none());
    assert_eq!(saved.worktree_source.unwrap().id(), worktree.id());
    session
        .record(Event::Worktree(Some(worktree.clone())))
        .unwrap();
    let saved = session.workspace_snapshot().unwrap();
    assert_eq!(saved.worktree.unwrap().id(), worktree.id());
    assert!(saved.worktree_source.is_none());
    session.record(Event::WorktreePruned).unwrap();
    crate::test_support::invalidate(&store, &session.id);
    let saved = session.workspace_snapshot().unwrap();
    assert!(saved.worktree.is_none());
    assert!(saved.worktree_source.is_none());
    assert!(session.snapshot().is_err());
}

#[test]
fn legacy_databases_backfill_workspace_metadata() {
    let workspace = Workspace::new();
    let source = workspace.store();
    let session = source
        .create(SessionProvider::Injected, None, history())
        .unwrap();
    let snapshot = session.snapshot().unwrap();
    let policy = WorkspacePolicy::workspace(workspace.path.clone()).unwrap();
    let storage = policy.session_storage("legacy-workspaces").unwrap();
    let env = unsafe {
        heed::EnvOpenOptions::new()
            .map_size(100 * 1024 * 1024 * 1024)
            .max_dbs(7)
            .open(storage.path())
            .unwrap()
    };
    let mut transaction = env.write_txn().unwrap();
    let database: Database<Str, Bytes> = env
        .create_database(&mut transaction, Some("session_snapshots"))
        .unwrap();
    database
        .put(
            &mut transaction,
            &snapshot.id,
            &serde_json::to_vec(&snapshot).unwrap(),
        )
        .unwrap();
    transaction.commit().unwrap();
    drop(env);
    let migrated = SessionStore::open(&policy, "legacy-workspaces").unwrap();
    let access = migrated.access().unwrap();
    let transaction = access.current.env.read_txn().unwrap();
    let saved = access
        .current
        .workspace_index
        .get(&transaction, &snapshot.id)
        .unwrap();
    assert!(saved.worktree.is_none());
    assert!(saved.worktree_source.is_none());
    assert_eq!(
        serde_json::to_value(access.current.snapshot(&transaction, &snapshot.id).unwrap()).unwrap(),
        serde_json::to_value(&snapshot).unwrap()
    );
}
