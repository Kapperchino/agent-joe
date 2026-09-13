use super::*;
use crate::session::{
    Event, ResumableSession, Session,
    artifacts::ArtifactRange,
    tests::{Workspace, history, save_output},
};
use clients::llm::{Message, SessionProvider};

fn store(workspace: &Workspace) -> Arc<SessionStore> {
    SessionStore::open_with_capacity(
        &WorkspacePolicy::workspace(workspace.path.clone()).unwrap(),
        "sessions",
        Capacity::new(2 * 1024 * 1024).unwrap(),
    )
    .unwrap()
}

fn create(store: &Arc<SessionStore>) -> Arc<Session> {
    store
        .create(SessionProvider::Injected, None, history())
        .unwrap()
}

fn rotate(store: &SessionStore) {
    store.access().unwrap().rotate(store, None).unwrap();
}

fn fill(store: &SessionStore, bytes: usize) {
    let access = store.access().unwrap();
    let bytes = bytes.saturating_sub(access.current.used_bytes());
    let mut transaction = access.current.env.write_txn().unwrap();
    access
        .current
        .artifacts
        .put(&mut transaction, "capacity-fixture", &vec![b'x'; bytes])
        .unwrap();
    transaction.commit().unwrap();
}

#[test]
fn default_capacity_is_one_hundred_gib_with_ten_percent_headroom() {
    let workspace = Workspace::new();
    let store = workspace.store();
    let access = store.access().unwrap();
    assert_eq!(
        access.current.env.info().map_size as u64,
        100 * 1024 * 1024 * 1024
    );
    assert_eq!(store.capacity.rotate_at as u64, 90 * 1024 * 1024 * 1024);
    assert!(
        std::fs::metadata(access.current.storage.path().join("data.mdb"))
            .unwrap()
            .len()
            < 1024 * 1024
    );
}

#[test]
fn automatic_rotation_keeps_current_and_old_searchable_then_archives_the_oldest() {
    let workspace = Workspace::new();
    let store = store(&workspace);
    let first = create(&store);
    let first_id = first.id.clone();
    drop(first);
    fill(&store, 1900 * 1024);
    let second = create(&store);
    let second_id = second.id.clone();
    drop(second);
    assert_eq!(
        store.access().unwrap().layout,
        Layout {
            current: 1,
            old: Some(0)
        }
    );
    let choices = store
        .resume_choices(&SessionProvider::Injected, None)
        .unwrap();
    assert_eq!(choices.len(), 2);
    assert!(choices.iter().any(|choice| choice.id == first_id));
    assert!(choices.iter().any(|choice| choice.id == second_id));
    let original = std::fs::read(store.storage.path().join("data.mdb")).unwrap();
    fill(&store, 1900 * 1024);
    let third = create(&store);
    let third_id = third.id.clone();
    drop(third);
    assert_eq!(
        store.access().unwrap().layout,
        Layout {
            current: 2,
            old: Some(1)
        }
    );
    let choices = store
        .resume_choices(&SessionProvider::Injected, None)
        .unwrap();
    assert_eq!(choices.len(), 2);
    assert!(choices.iter().all(|choice| choice.id != first_id));
    assert!(choices.iter().any(|choice| choice.id == second_id));
    assert!(choices.iter().any(|choice| choice.id == third_id));
    let archive = store
        .storage
        .read_file("archive-00000000000000000000.mdb.zst")
        .unwrap()
        .unwrap();
    assert!(archive.metadata().unwrap().len() < original.len() as u64);
    assert_eq!(zstd::stream::decode_all(archive).unwrap(), original);
    assert!(!store.storage.path().join("data.mdb").exists());
    let policy = WorkspacePolicy::workspace(workspace.path.clone()).unwrap();
    assert!(ResumableSession::new(&store, &first_id, &policy, &SessionProvider::Injected).is_err());
    drop(store);
    let reopened = self::store(&workspace);
    assert_eq!(reopened.list().unwrap().len(), 2);
    assert!(!reopened.storage.path().join("data.mdb").exists());
    assert!(
        ResumableSession::new(&reopened, &second_id, &policy, &SessionProvider::Injected)
            .unwrap()
            .resume()
            .is_ok()
    );
}

#[test]
fn active_conversations_keep_ownership_events_and_worker_artifacts_across_rotations() {
    let workspace = Workspace::new();
    let store = store(&workspace);
    let parent = create(&store);
    let worker = store
        .create(
            SessionProvider::Injected,
            Some(parent.id.clone()),
            history(),
        )
        .unwrap();
    save_output(&worker, &"worker output ".repeat(1000));
    let artifact = worker.snapshot().unwrap().artifacts[0].clone();
    let sequence = worker.snapshot().unwrap().sequence;
    rotate(&store);
    rotate(&store);
    assert_eq!(worker.snapshot().unwrap().sequence, sequence);
    assert_eq!(store.list().unwrap().len(), 2);
    for session in [&parent, &worker] {
        let page = session
            .read_artifact(&artifact.id, ArtifactRange::new(0, 4096).unwrap())
            .unwrap();
        assert!(page.content.starts_with("worker output "));
        session
            .record(Event::History(vec![Message::new("after rotation".into())]))
            .unwrap();
    }
    let policy = WorkspacePolicy::workspace(workspace.path.clone()).unwrap();
    assert!(
        ResumableSession::new(&store, &parent.id, &policy, &SessionProvider::Injected).is_err()
    );
    let access = store.access().unwrap();
    let transaction = access.current.env.read_txn().unwrap();
    assert_eq!(
        access
            .current
            .events
            .prefix_iter(&transaction, &format!("{}:", worker.id))
            .unwrap()
            .count() as u64,
        sequence + 1
    );
}

#[test]
fn resuming_an_old_session_moves_its_conversation_into_the_current_database() {
    let workspace = Workspace::new();
    let store = store(&workspace);
    let parent = create(&store);
    let worker = store
        .create(
            SessionProvider::Injected,
            Some(parent.id.clone()),
            history(),
        )
        .unwrap();
    save_output(&worker, &"saved output ".repeat(1000));
    let artifact = worker.snapshot().unwrap().artifacts[0].clone();
    let id = parent.id.clone();
    let worker_id = worker.id.clone();
    drop(worker);
    drop(parent);
    rotate(&store);
    assert!(!store.access().unwrap().current.contains(&id).unwrap());
    let policy = WorkspacePolicy::workspace(workspace.path.clone()).unwrap();
    let resumed = ResumableSession::new(&store, &id, &policy, &SessionProvider::Injected)
        .unwrap()
        .resume()
        .unwrap();
    assert!(
        store
            .access()
            .unwrap()
            .current
            .contains(&worker_id)
            .unwrap()
    );
    rotate(&store);
    assert!(
        resumed
            .read_artifact(&artifact.id, ArtifactRange::new(0, 4096).unwrap())
            .unwrap()
            .content
            .starts_with("saved output ")
    );
    assert_eq!(
        store
            .resume_choices(&SessionProvider::Injected, None)
            .unwrap()
            .len(),
        1
    );
    let fork = resumed.fork().unwrap();
    assert!(
        fork.read_artifact(&artifact.id, ArtifactRange::new(0, 4096).unwrap())
            .is_ok()
    );
}

#[test]
fn map_full_retries_only_the_aborted_database_transaction() {
    let workspace = Workspace::new();
    let store = store(&workspace);
    let session = create(&store);
    fill(&store, 1600 * 1024);
    let message = Message::new("x".repeat(300 * 1024));
    session.record(Event::History(vec![message])).unwrap();
    assert_eq!(store.access().unwrap().layout.current, 1);
    assert_eq!(session.snapshot().unwrap().sequence, 2);
    assert_eq!(session.snapshot().unwrap().history.len(), 3);
    let access = store.access().unwrap();
    let transaction = access.current.env.read_txn().unwrap();
    assert_eq!(access.current.events.len(&transaction).unwrap(), 2);
    let old = access.old.as_ref().unwrap();
    let transaction = old.env.read_txn().unwrap();
    assert_eq!(old.events.len(&transaction).unwrap(), 1);
}

#[test]
fn an_oversized_write_keeps_the_last_snapshot_and_event_sequence() {
    let workspace = Workspace::new();
    let store = store(&workspace);
    let session = create(&store);
    assert!(
        session
            .record(Event::History(vec![Message::new(
                "x".repeat(2 * 1024 * 1024)
            )]))
            .is_err()
    );
    assert_eq!(session.snapshot().unwrap().sequence, 1);
    assert_eq!(session.snapshot().unwrap().history.len(), 2);
    assert!(
        store
            .create(
                SessionProvider::Injected,
                None,
                vec![Message::new("x".repeat(2 * 1024 * 1024))]
            )
            .is_err()
    );
    let access = store.access().unwrap();
    let transaction = access.current.env.read_txn().unwrap();
    assert_eq!(access.current.events.len(&transaction).unwrap(), 1);
    assert_eq!(access.current.snapshots.len(&transaction).unwrap(), 1);
    assert_eq!(access.current.owners.len(&transaction).unwrap(), 1);
}

#[test]
fn archive_failure_keeps_both_searchable_databases_and_can_be_retried() {
    let workspace = Workspace::new();
    let store = store(&workspace);
    let first = create(&store);
    drop(first);
    rotate(&store);
    let second = create(&store);
    drop(second);
    let blocked = store
        .storage
        .path()
        .join("archive-00000000000000000000.mdb.zst.tmp");
    std::fs::create_dir(&blocked).unwrap();
    assert!(store.access().unwrap().rotate(&store, None).is_err());
    assert_eq!(store.access().unwrap().layout.current, 1);
    assert_eq!(store.list().unwrap().len(), 2);
    assert!(store.storage.path().join("data.mdb").exists());
    std::fs::remove_dir(blocked).unwrap();
    rotate(&store);
    assert_eq!(store.list().unwrap().len(), 1);
}

#[test]
fn interrupted_retirement_finishes_on_reopen() {
    let workspace = Workspace::new();
    let store = store(&workspace);
    drop(create(&store));
    rotate(&store);
    let second = create(&store);
    let id = second.id.clone();
    drop(second);
    {
        let access = store.access().unwrap();
        Catalog::rotate(&access, &store.storage, store.capacity, None).unwrap();
    }
    assert!(matches!(
        StorageState::load(&store.storage).unwrap(),
        StorageState::Retiring { .. }
    ));
    assert!(store.storage.path().join("data.mdb").exists());
    drop(store);
    let reopened = self::store(&workspace);
    assert_eq!(reopened.list().unwrap()[0].id, id);
    assert!(!reopened.storage.path().join("data.mdb").exists());
    assert!(matches!(
        StorageState::load(&reopened.storage).unwrap(),
        StorageState::Ready(_)
    ));
}

#[test]
fn another_process_rotates_without_invalidating_active_session_handles() {
    let workspace = Workspace::new();
    let store = store(&workspace);
    let session = create(&store);
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "session::generations::tests::rotation_process_fixture",
            "--nocapture",
        ])
        .env("JOE_SESSION_ROTATION_WORKSPACE", &workspace.path)
        .env("JOE_SESSION_ROTATION_ID", &session.id)
        .status()
        .unwrap();
    assert!(status.success());
    session
        .record(Event::History(vec![Message::new(
            "after another process rotated".into(),
        )]))
        .unwrap();
    assert_eq!(session.snapshot().unwrap().sequence, 2);
    assert_eq!(store.access().unwrap().layout.current, 2);
}

#[test]
fn rotation_process_fixture() {
    if let Some(path) = std::env::var_os("JOE_SESSION_ROTATION_WORKSPACE") {
        let policy = WorkspacePolicy::workspace(path.into()).unwrap();
        let store = SessionStore::open_with_capacity(
            &policy,
            "sessions",
            Capacity::new(2 * 1024 * 1024).unwrap(),
        )
        .unwrap();
        let id = std::env::var("JOE_SESSION_ROTATION_ID").unwrap();
        rotate(&store);
        rotate(&store);
        assert!(
            ResumableSession::new(&store, &id, &policy, &SessionProvider::Injected)
                .err()
                .unwrap()
                .to_string()
                .contains("already open")
        );
    }
}

#[test]
fn map_full_preserves_the_tool_completion_and_archives_its_output_once() {
    use crate::session::tests::{batch, success};
    use tools::tool_defs::{ToolEffect, ToolResult};
    let workspace = Workspace::new();
    let store = store(&workspace);
    let session = create(&store);
    let pending = batch();
    let operation = pending.operations[0].clone();
    session.record(Event::Prepared(pending)).unwrap();
    session
        .record(Event::Intent {
            operation: operation.id.clone(),
            effect: ToolEffect::Write,
        })
        .unwrap();
    fill(&store, 1600 * 1024);
    let content = "output".repeat(120 * 1024);
    let result = session
        .complete_tool(
            operation.id.clone(),
            ToolResult {
                outcome: Ok(content.clone()),
                ..success(&operation)
            },
        )
        .unwrap();
    assert_eq!(store.access().unwrap().layout.current, 1);
    let snapshot = session.snapshot().unwrap();
    assert_eq!(snapshot.sequence, 4);
    assert_eq!(snapshot.artifacts.len(), 1);
    assert!(result.outcome.unwrap().contains(&snapshot.artifacts[0].id));
    let page = session
        .read_artifact(
            &snapshot.artifacts[0].id,
            ArtifactRange::new(0, 4096).unwrap(),
        )
        .unwrap();
    assert_eq!(page.content, content[..4096]);
    let access = store.access().unwrap();
    let transaction = access.current.env.read_txn().unwrap();
    assert_eq!(access.current.artifacts.len(&transaction).unwrap(), 1);
    assert_eq!(access.current.events.len(&transaction).unwrap(), 4);
}

#[test]
fn subsequent_rotations_keep_existing_archives_and_only_two_searchable_generations() {
    let workspace = Workspace::new();
    let store = store(&workspace);
    drop(create(&store));
    rotate(&store);
    drop(create(&store));
    rotate(&store);
    let first_archive = std::fs::read(
        store
            .storage
            .path()
            .join("archive-00000000000000000000.mdb.zst"),
    )
    .unwrap();
    drop(create(&store));
    rotate(&store);
    assert_eq!(
        std::fs::read(
            store
                .storage
                .path()
                .join("archive-00000000000000000000.mdb.zst")
        )
        .unwrap(),
        first_archive
    );
    assert!(
        store
            .storage
            .read_file("archive-00000000000000000001.mdb.zst")
            .unwrap()
            .is_some()
    );
    assert!(
        !store
            .storage
            .path()
            .join("generation-00000000000000000001")
            .exists()
    );
    assert_eq!(store.list().unwrap().len(), 1);
    assert_eq!(
        store.access().unwrap().layout,
        Layout {
            current: 3,
            old: Some(2)
        }
    );
}

#[test]
fn concurrent_stores_can_rotate_and_commit_independently() {
    std::thread::scope(|threads| {
        let workers = (0..8)
            .map(|_| {
                threads.spawn(|| {
                    for _ in 0..4 {
                        let workspace = Workspace::new();
                        let store = store(&workspace);
                        let session = create(&store);
                        for _ in 0..4 {
                            session
                                .record(Event::History(vec![Message::new(
                                    "concurrent write".into(),
                                )]))
                                .unwrap();
                            rotate(&store);
                        }
                        assert_eq!(session.snapshot().unwrap().sequence, 5);
                    }
                })
            })
            .collect::<Vec<_>>();
        for worker in workers {
            worker.join().unwrap();
        }
    });
}

#[test]
fn the_larger_default_reopens_a_full_legacy_database_without_losing_sessions() {
    let workspace = Workspace::new();
    let store = store(&workspace);
    let session = create(&store);
    let id = session.id.clone();
    drop(session);
    fill(&store, 1900 * 1024);
    drop(store);
    let reopened = workspace.store();
    let policy = WorkspacePolicy::workspace(workspace.path.clone()).unwrap();
    let session = ResumableSession::new(&reopened, &id, &policy, &SessionProvider::Injected)
        .unwrap()
        .resume()
        .unwrap();
    session
        .record(Event::History(vec![Message::new(
            "continued legacy session".into(),
        )]))
        .unwrap();
    assert_eq!(session.snapshot().unwrap().sequence, 3);
    assert_eq!(reopened.access().unwrap().layout.current, 0);
}

#[test]
fn a_missing_generation_is_reported_instead_of_becoming_an_empty_database() {
    let workspace = Workspace::new();
    let store = store(&workspace);
    drop(create(&store));
    rotate(&store);
    let current = generation(&store.storage, 1).unwrap();
    drop(store);
    current.remove_file("data.mdb").unwrap();
    let policy = WorkspacePolicy::workspace(workspace.path.clone()).unwrap();
    assert!(
        SessionStore::open_with_capacity(
            &policy,
            "sessions",
            Capacity::new(2 * 1024 * 1024).unwrap()
        )
        .err()
        .unwrap()
        .to_string()
        .contains("generation 1 is missing")
    );
}

#[test]
fn invalid_manifests_are_rejected_before_generations_are_modified() {
    let workspace = Workspace::new();
    let store = store(&workspace);
    drop(create(&store));
    rotate(&store);
    let storage = store.storage.clone();
    let original = std::fs::read(storage.path().join("data.mdb")).unwrap();
    let current_path = storage
        .path()
        .join("generation-00000000000000000001/data.mdb");
    let current = std::fs::read(&current_path).unwrap();
    drop(store);
    let policy = WorkspacePolicy::workspace(workspace.path.clone()).unwrap();
    let states = [
        serde_json::json!({"Ready": {"current": 1, "old": null}}),
        serde_json::json!({"Ready": {"current": 2, "old": 0}}),
        serde_json::json!({"Retiring": {"layout": {"current": 1, "old": 0}, "archived": 0}}),
        serde_json::json!({"Retiring": {"layout": {"current": 0, "old": null}, "archived": u64::MAX}}),
    ];
    for state in states {
        storage
            .replace_file(
                MANIFEST,
                &serde_json::to_vec(&serde_json::json!({"version": 1, "state": state})).unwrap(),
            )
            .unwrap();
        assert!(
            SessionStore::open_with_capacity(
                &policy,
                "sessions",
                Capacity::new(2 * 1024 * 1024).unwrap()
            )
            .is_err()
        );
        assert_eq!(
            std::fs::read(storage.path().join("data.mdb")).unwrap(),
            original
        );
        assert_eq!(std::fs::read(&current_path).unwrap(), current);
    }
}
