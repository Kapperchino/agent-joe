use super::*;
use crate::{Event, SessionStore, test_support::Workspace, tests::history};
use clients::llm::Message;
use common_models::tui_models::Lifecycle;
use utils::workspace::WorkspacePolicy;

#[test]
fn resume_choices_use_compact_records_for_large_histories() {
    let workspace = Workspace::new();
    let store = workspace.store();
    for index in 0..16 {
        let history = history()
            .into_iter()
            .chain((0..64).map(|_| Message::new_assistant("é".repeat(8192))))
            .chain([Message::new_assistant(format!("Saved response {index}"))])
            .collect();
        drop(
            store
                .create(SessionProvider::Injected, None, history)
                .unwrap(),
        );
    }
    let start = std::time::Instant::now();
    let mut expected = store
        .list()
        .unwrap()
        .into_iter()
        .filter_map(|snapshot| snapshot.summary())
        .collect::<Vec<_>>();
    expected.sort_by(|left, right| {
        right
            .updated_at
            .cmp(&left.updated_at)
            .then_with(|| left.id.cmp(&right.id))
    });
    let full_snapshot_elapsed = start.elapsed();
    let start = std::time::Instant::now();
    let choices = store
        .resume_choices(&SessionProvider::Injected, None)
        .unwrap();
    let indexed_elapsed = start.elapsed();
    assert_eq!(choices.len(), 16);
    assert_eq!(
        serde_json::to_value(&choices).unwrap(),
        serde_json::to_value(&expected).unwrap()
    );
    let access = store.access().unwrap();
    let transaction = access.current.env.read_txn().unwrap();
    let bytes = access
        .current
        .session_index
        .entries
        .iter(&transaction)
        .unwrap()
        .map(|entry| entry.unwrap().1.len())
        .sum::<usize>();
    assert!(bytes < 16 * 2048);
    println!(
        "16 sessions with 16 MiB of history: full snapshots {full_snapshot_elapsed:?}, indexed choices {indexed_elapsed:?}, index {bytes} bytes"
    );
}

#[test]
fn resume_choices_track_committed_history_and_status_without_decoding_snapshots() {
    let workspace = Workspace::new();
    let store = workspace.store();
    let session = store
        .create(
            SessionProvider::Injected,
            None,
            vec![Message::new("context".into())],
        )
        .unwrap();
    assert!(
        store
            .resume_choices(&SessionProvider::Injected, None)
            .unwrap()
            .is_empty()
    );
    session
        .record(Event::History(vec![
            Message::new("  Fix\n résumé  search  ".into()),
            Message::new_assistant("Résumé result ".repeat(100)),
        ]))
        .unwrap();
    session
        .record(Event::Status {
            turn: "saved-turn".into(),
            state: Lifecycle::Failed,
            detail: None,
        })
        .unwrap();
    let snapshot = session.snapshot().unwrap();
    let expected = snapshot.summary().unwrap();
    let choices = store
        .resume_choices(&SessionProvider::Injected, None)
        .unwrap();
    assert_eq!(choices.len(), 1);
    assert_eq!(choices[0].title, "Fix résumé search");
    assert_eq!(choices[0].preview.chars().count(), 500);
    assert_eq!(choices[0].status, Lifecycle::Failed);
    assert_eq!(
        serde_json::to_value(&choices[0]).unwrap(),
        serde_json::to_value(&expected).unwrap()
    );
    let id = session.id.clone();
    drop(session);
    crate::test_support::invalidate(&store, &id);
    assert!(store.list().is_err());
    assert_eq!(
        serde_json::to_value(
            store
                .resume_choices(&SessionProvider::Injected, None)
                .unwrap()
        )
        .unwrap(),
        serde_json::to_value(&choices).unwrap()
    );
    drop(store);
    let reopened = workspace.store();
    assert_eq!(
        serde_json::to_value(
            reopened
                .resume_choices(&SessionProvider::Injected, None)
                .unwrap()
        )
        .unwrap(),
        serde_json::to_value(&choices).unwrap()
    );
}

#[test]
fn legacy_databases_backfill_resume_summaries_without_changing_saved_sessions() {
    let workspace = Workspace::new();
    let source = workspace.store();
    let root = source
        .create(SessionProvider::Injected, None, history())
        .unwrap();
    let _worker = source
        .create(SessionProvider::Injected, Some(root.id.clone()), history())
        .unwrap();
    let _empty = source
        .create(
            SessionProvider::Injected,
            None,
            vec![Message::new("context".into())],
        )
        .unwrap();
    let mut snapshots = source.list().unwrap();
    snapshots
        .iter_mut()
        .for_each(|snapshot| snapshot.updated_at = None);
    let expected = snapshots
        .iter()
        .find(|snapshot| snapshot.id == root.id)
        .unwrap()
        .summary()
        .unwrap();
    let policy = WorkspacePolicy::workspace(workspace.path.clone()).unwrap();
    let storage = policy.session_storage("legacy-summaries").unwrap();
    let env = unsafe {
        heed::EnvOpenOptions::new()
            .map_size(64 * 1024 * 1024)
            .max_dbs(6)
            .open(storage.path())
            .unwrap()
    };
    let mut transaction = env.write_txn().unwrap();
    let database: Database<Str, Bytes> = env
        .create_database(&mut transaction, Some("session_snapshots"))
        .unwrap();
    for snapshot in &snapshots {
        let mut value = serde_json::to_value(snapshot).unwrap();
        value.as_object_mut().unwrap().remove("updated_at");
        database
            .put(
                &mut transaction,
                &snapshot.id,
                &serde_json::to_vec(&value).unwrap(),
            )
            .unwrap();
    }
    transaction.commit().unwrap();
    drop(env);
    let migrated = SessionStore::open(&policy, "legacy-summaries").unwrap();
    let choices = migrated
        .resume_choices(&SessionProvider::Injected, None)
        .unwrap();
    assert_eq!(choices.len(), 1);
    assert_eq!(
        serde_json::to_value(&choices[0]).unwrap(),
        serde_json::to_value(expected).unwrap()
    );
    let access = migrated.access().unwrap();
    let transaction = access.current.env.read_txn().unwrap();
    let listings = access.current.session_index.list(&transaction).unwrap();
    assert_eq!(listings.len(), 3);
    assert_eq!(
        listings
            .iter()
            .filter(|listing| listing.summary.is_none())
            .count(),
        2
    );
    assert_eq!(access.current.events.len(&transaction).unwrap(), 0);
    for snapshot in snapshots {
        let saved = access.current.snapshot(&transaction, &snapshot.id).unwrap();
        assert_eq!(
            serde_json::to_value(saved).unwrap(),
            serde_json::to_value(snapshot).unwrap()
        );
    }
}
