use super::*;
use crate::git::tests::Fixture;

#[test]
fn dirty_baseline_concurrent_edits_and_guarded_undo_survive_serialization() {
    let fixture = Fixture::new();
    fixture.write("file", "committed\n");
    fixture.stage("file");
    fixture.commit();
    fixture.write("file", "staged\n");
    fixture.stage("file");
    fixture.write("file", "user baseline\n");
    fixture.write("untracked", "existing untracked\n");
    let index = std::fs::read(fixture.root.join(".git/index")).unwrap();
    let tracker = ChangeTracker::default();
    tracker.start(&fixture.workspace).unwrap();
    let before = fixture.workspace.file_version(Path::new("file")).unwrap();
    let edit = FileEdit::new(
        &fixture.workspace,
        Path::new("file"),
        before.clone(),
        before.with_text("Joe result\n".into()),
    )
    .unwrap();
    let record = tracker.apply(&fixture.workspace, vec![edit]).unwrap();
    let review = tracker.review(&fixture.workspace).unwrap();
    assert!(review.baseline_staged.contains("+staged"));
    assert!(
        review
            .changes
            .iter()
            .any(|change| change.task_diff.contains("-user baseline\n+Joe result"))
    );
    assert!(
        !review
            .changes
            .iter()
            .any(|change| change.path == Path::new("untracked"))
    );
    fixture.write("file", "concurrent user change\n");
    let review = tracker.review(&fixture.workspace).unwrap();
    assert!(matches!(
        review.changes[0].ownership,
        ChangeOwnership::JoeAndExternal
    ));
    assert!(tracker.undo(&fixture.workspace, &record.id).is_err());
    assert_eq!(
        fixture.workspace.read(Path::new("file")).unwrap(),
        "concurrent user change\n"
    );
    fixture.write("file", "Joe result\n");
    let saved = serde_json::to_vec(&tracker.snapshot().unwrap()).unwrap();
    let restored = ChangeTracker::restored(serde_json::from_slice(&saved).unwrap(), None);
    restored.undo(&fixture.workspace, &record.id).unwrap();
    assert_eq!(
        fixture.workspace.read(Path::new("file")).unwrap(),
        "user baseline\n"
    );
    assert_eq!(
        index,
        std::fs::read(fixture.root.join(".git/index")).unwrap()
    );
}

#[test]
fn stale_reads_preflight_all_files_and_duplicate_paths_do_not_overwrite() {
    let fixture = Fixture::new();
    fixture.write("one", "one\n");
    fixture.write("two", "two\n");
    let tracker = ChangeTracker::default();
    tracker.start(&fixture.workspace).unwrap();
    let edit = |path: &str, after: &str| {
        let before = fixture.workspace.file_version(Path::new(path)).unwrap();
        FileEdit::new(
            &fixture.workspace,
            Path::new(path),
            before.clone(),
            before.with_text(after.into()),
        )
        .unwrap()
    };
    let one = edit("one", "Joe one\n");
    let two = edit("two", "Joe two\n");
    fixture.write("two", "user two\n");
    assert!(
        tracker
            .apply(&fixture.workspace, vec![one.clone(), two])
            .is_err()
    );
    assert_eq!(fixture.workspace.read(Path::new("one")).unwrap(), "one\n");
    assert!(
        tracker
            .apply(&fixture.workspace, vec![one.clone(), one])
            .is_err()
    );
    let one = edit("one", "Joe one\n");
    let mut alias = one.clone();
    alias.path = fixture.root.join("one");
    assert!(tracker.apply(&fixture.workspace, vec![one, alias]).is_err());
    let directory = FileEdit::new(
        &fixture.workspace,
        Path::new("new-directory"),
        FileVersion::Missing,
        FileVersion::Missing.with_text("ordinary file\n".into()),
    )
    .unwrap();
    let child = FileEdit::new(
        &fixture.workspace,
        Path::new("new-directory/child"),
        FileVersion::Missing,
        FileVersion::Missing.with_text("child file\n".into()),
    )
    .unwrap();
    assert!(
        tracker
            .apply(&fixture.workspace, vec![directory, child])
            .is_err()
    );
    assert!(!fixture.root.join("new-directory").exists());
    assert!(tracker.snapshot().unwrap().records.is_empty());
    assert!(
        tracker
            .apply(&fixture.workspace, vec![edit("two", "stale replacement\n")])
            .is_err()
    );
    let version = fixture.workspace.file_version(Path::new("two")).unwrap();
    tracker
        .observe(&fixture.workspace, Path::new("two"), version)
        .unwrap();
    tracker
        .apply(&fixture.workspace, vec![edit("two", "fresh replacement\n")])
        .unwrap();
}

struct ConcurrentWriter {
    root: PathBuf,
    saved: Mutex<Option<ChangeSnapshot>>,
}

impl ChangeStore for ConcurrentWriter {
    fn save(&self, snapshot: &ChangeSnapshot) -> anyhow::Result<()> {
        *self.saved.lock().unwrap() = Some(snapshot.clone());
        if snapshot
            .records
            .last()
            .is_some_and(|record| record.applied == [PathBuf::from("one")])
        {
            std::fs::write(self.root.join("two"), "concurrent write\n")?;
        }
        Ok(())
    }
}

#[test]
fn failure_halfway_is_journaled_and_remaining_staged_files_are_removed() {
    let fixture = Fixture::new();
    fixture.write("one", "old one\n");
    fixture.write("two", "old two\n");
    let store = Arc::new(ConcurrentWriter {
        root: fixture.root.clone(),
        saved: Mutex::new(None),
    });
    let tracker = ChangeTracker::restored(Default::default(), Some(store.clone()));
    tracker.start(&fixture.workspace).unwrap();
    let edits = ["one", "two"]
        .into_iter()
        .map(|path| {
            let before = fixture.workspace.file_version(Path::new(path)).unwrap();
            FileEdit::new(
                &fixture.workspace,
                Path::new(path),
                before.clone(),
                before.with_text("new\n".into()),
            )
            .unwrap()
        })
        .collect();
    let error = tracker.apply(&fixture.workspace, edits).unwrap_err();
    assert!(error.to_string().contains("applied paths"));
    assert_eq!(fixture.workspace.read(Path::new("one")).unwrap(), "new\n");
    assert_eq!(
        fixture.workspace.read(Path::new("two")).unwrap(),
        "concurrent write\n"
    );
    let saved = store.saved.lock().unwrap().clone().unwrap();
    let record = saved.records.last().unwrap();
    assert!(matches!(record.state, EditState::Failed { .. }));
    assert_eq!(record.applied, [PathBuf::from("one")]);
    assert_eq!(record.in_flight.as_deref(), Some(Path::new("two")));
    assert!(tracker.undo(&fixture.workspace, &record.id).is_err());
    assert!(std::fs::read_dir(&fixture.root).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".joe-write-")
    }));
}

#[test]
fn accepted_user_edits_between_joe_edits_keep_separate_attribution() {
    let fixture = Fixture::new();
    fixture.write("file", "baseline\n");
    let tracker = ChangeTracker::default();
    tracker.start(&fixture.workspace).unwrap();
    let edit = |text: &str| {
        let before = fixture.workspace.file_version(Path::new("file")).unwrap();
        FileEdit::new(
            &fixture.workspace,
            Path::new("file"),
            before.clone(),
            before.with_text(text.into()),
        )
        .unwrap()
    };
    tracker
        .apply(&fixture.workspace, vec![edit("first Joe\n")])
        .unwrap();
    fixture.write("file", "first Joe\nuser addition\n");
    tracker
        .observe(
            &fixture.workspace,
            Path::new("file"),
            fixture.workspace.file_version(Path::new("file")).unwrap(),
        )
        .unwrap();
    let second = tracker
        .apply(
            &fixture.workspace,
            vec![edit("second Joe\nuser addition\n")],
        )
        .unwrap();
    let review = tracker.review(&fixture.workspace).unwrap();
    assert!(matches!(
        review.changes[0].ownership,
        ChangeOwnership::JoeAndExternal
    ));
    assert!(!review.changes[0].joe_diff.contains("+user addition"));
    tracker.undo(&fixture.workspace, &second.id).unwrap();
    assert_eq!(
        fixture.workspace.read(Path::new("file")).unwrap(),
        "first Joe\nuser addition\n"
    );
}

#[tokio::test]
async fn a_missing_copy_or_move_source_cannot_delete_the_destination() {
    let fixture = Fixture::new();
    fixture.write("destination", "preserve\n");
    let scope = crate::execution::ExecutionScope::with_workspace(
        WorkspacePolicy::workspace(fixture.root.clone()).unwrap(),
    );
    scope
        .enter(async {
            assert!(
                crate::files::Files::copy_file(Path::new("missing"), Path::new("destination"))
                    .await
                    .is_err()
            );
            assert!(
                crate::files::Files::rename_file(
                    Path::new("missing"),
                    Path::new("new-destination")
                )
                .await
                .is_err()
            );
        })
        .await;
    assert_eq!(
        fixture.workspace.read(Path::new("destination")).unwrap(),
        "preserve\n"
    );
    assert!(!fixture.root.join("new-destination").exists());
}

#[test]
fn index_blob_modes_and_flags_are_captured_separately_from_working_files() {
    let fixture = Fixture::new();
    fixture.write("file", "base\n");
    fixture.stage("file");
    fixture.commit();
    fixture.write("file", "staged at task start\n");
    fixture.stage("file");
    fixture.write("file", "working file\n");
    let tracker = ChangeTracker::default();
    tracker.start(&fixture.workspace).unwrap();
    let baseline = tracker.snapshot().unwrap().baseline.unwrap();
    fixture.write("file", "concurrently staged\n");
    fixture.stage("file");
    fixture.write("file", "working file\n");
    let review = tracker.review(&fixture.workspace).unwrap();
    assert!(review.changes.is_empty());
    assert_eq!(review.index_changes.len(), 1);
    let change = &review.index_changes[0];
    assert_eq!(change.before.as_ref().unwrap().blob, baseline.index[0].blob);
    assert_ne!(
        change.before.as_ref().unwrap().blob,
        change.after.as_ref().unwrap().blob
    );
    assert!(review.edits.is_empty());
}

#[derive(Default)]
struct LostCompletion {
    saved: Mutex<ChangeSnapshot>,
}

impl ChangeStore for LostCompletion {
    fn save(&self, snapshot: &ChangeSnapshot) -> anyhow::Result<()> {
        match snapshot
            .records
            .iter()
            .any(|record| !record.applied.is_empty())
        {
            true => Err(anyhow::anyhow!(
                "Simulated storage failure after replacement"
            )),
            false => {
                *self.saved.lock().unwrap() = snapshot.clone();
                Ok(())
            }
        }
    }
}

#[test]
fn uncommitted_completion_retains_the_in_flight_path_for_recovery() {
    let fixture = Fixture::new();
    fixture.write("file", "before\n");
    let store = Arc::new(LostCompletion::default());
    let tracker = ChangeTracker::restored(Default::default(), Some(store.clone()));
    tracker.start(&fixture.workspace).unwrap();
    let before = fixture.workspace.file_version(Path::new("file")).unwrap();
    let edit = FileEdit::new(
        &fixture.workspace,
        Path::new("file"),
        before.clone(),
        before.with_text("after\n".into()),
    )
    .unwrap();
    assert!(tracker.apply(&fixture.workspace, vec![edit]).is_err());
    assert_eq!(
        fixture.workspace.read(Path::new("file")).unwrap(),
        "after\n"
    );
    let saved = store.saved.lock().unwrap().clone();
    assert_eq!(
        saved.records[0].in_flight.as_deref(),
        Some(Path::new("file"))
    );
    assert!(saved.records[0].applied.is_empty());
    let recovered = ChangeTracker::restored(saved.clone(), None);
    let review = recovered.review(&fixture.workspace).unwrap();
    assert!(matches!(
        review.changes[0].ownership,
        ChangeOwnership::Uncertain
    ));
    assert!(
        recovered
            .undo(&fixture.workspace, &saved.records[0].id)
            .is_err()
    );
    assert_eq!(
        fixture.workspace.read(Path::new("file")).unwrap(),
        "after\n"
    );
}

#[test]
fn restored_edits_require_confirmed_paths_before_undo() {
    let fixture = Fixture::new();
    fixture.write("file", "before\n");
    let tracker = ChangeTracker::default();
    tracker.start(&fixture.workspace).unwrap();
    let before = fixture.workspace.file_version(Path::new("file")).unwrap();
    let edit = FileEdit::new(
        &fixture.workspace,
        Path::new("file"),
        before.clone(),
        before.with_text("after\n".into()),
    )
    .unwrap();
    let record = tracker.apply(&fixture.workspace, vec![edit]).unwrap();
    let mut missing_confirmation = tracker.snapshot().unwrap();
    missing_confirmation.records[0].applied.clear();
    let mut pending_confirmation = tracker.snapshot().unwrap();
    pending_confirmation.records[0].in_flight = Some(PathBuf::from("file"));
    for snapshot in [missing_confirmation, pending_confirmation] {
        let saved = serde_json::to_vec(&snapshot).unwrap();
        let restored = ChangeTracker::restored(serde_json::from_slice(&saved).unwrap(), None);
        assert!(restored.undo(&fixture.workspace, &record.id).is_err());
        assert_eq!(restored.snapshot().unwrap().records.len(), 1);
        assert_eq!(
            fixture.workspace.read(Path::new("file")).unwrap(),
            "after\n"
        );
    }
}
