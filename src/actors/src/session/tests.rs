use super::*;
use std::path::PathBuf;
use tools::tool_defs::{ToolId, ToolInvocation};

pub(crate) struct Workspace {
    pub path: PathBuf,
}

impl Workspace {
    pub fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "joe-m4-{}-{}",
            std::process::id(),
            common_models::runtime_ids::OperationId::new()
        ));
        std::fs::create_dir(&path).unwrap();
        Self { path }
    }

    pub fn store(&self) -> Arc<SessionStore> {
        open(&self.path)
    }
    fn resume(
        &self,
        store: &Arc<SessionStore>,
        id: &str,
        provider: &SessionProvider,
    ) -> anyhow::Result<Arc<Session>> {
        let policy = WorkspacePolicy::workspace(self.path.clone())?;
        ResumableSession::new(store, id, &policy, provider)?.resume()
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.path).unwrap();
    }
}

fn open(path: &std::path::Path) -> Arc<SessionStore> {
    SessionStore::open(
        &WorkspacePolicy::workspace(path.to_owned()).unwrap(),
        "sessions",
    )
    .unwrap()
}

pub(crate) fn invalidate(store: &SessionStore, id: &str) {
    let mut transaction = store.env.write_txn().unwrap();
    store
        .snapshots
        .put(&mut transaction, id, br#"{"version":999}"#)
        .unwrap();
    transaction.commit().unwrap();
}

fn history() -> Vec<Message> {
    vec![
        Message::new("workspace".into()),
        Message::new("Keep the user's existing changes and validate the fix".into()),
    ]
}

fn save_output(session: &Session, content: &str) -> ToolResult {
    let mut pending = batch();
    pending.operations.truncate(1);
    pending.assistant.content.truncate(1);
    let operation = pending.operations[0].clone();
    session.record(Event::Prepared(pending)).unwrap();
    session
        .record(Event::Intent {
            operation: operation.id.clone(),
            effect: ToolEffect::Read,
        })
        .unwrap();
    let result = session
        .complete_tool(
            operation.id.clone(),
            ToolResult {
                outcome: Ok(content.into()),
                ..success(&operation)
            },
        )
        .unwrap();
    let messages = session.snapshot().unwrap().pending.unwrap().messages();
    session.record(Event::History(messages.into())).unwrap();
    result
}

#[test]
fn full_artifacts_survive_restart_with_bounded_utf8_pages() {
    use super::artifacts::{ArtifactRange, INLINE_BYTES};
    let workspace = Workspace::new();
    let store = workspace.store();
    let session = store
        .create(SessionProvider::Injected, None, history())
        .unwrap();
    let content = format!(
        "start\n{}\nfinal diagnostic",
        "résumé 終わり\n".repeat(10_000)
    );
    let result = save_output(&session, &content);
    let preview = result.outcome.unwrap();
    assert!(preview.len() <= INLINE_BYTES);
    assert!(preview.contains("final diagnostic"));
    let artifact = session.snapshot().unwrap().artifacts[0].clone();
    assert!(preview.contains(&artifact.id));
    let id = session.id.clone();
    let unrelated = store
        .create(SessionProvider::Injected, None, history())
        .unwrap();
    assert!(
        unrelated
            .read_artifact(&artifact.id, ArtifactRange::new(0, 4096).unwrap())
            .is_err()
    );
    drop(unrelated);
    drop(session);
    drop(store);
    let store = workspace.store();
    let session = workspace
        .resume(&store, &id, &SessionProvider::Injected)
        .unwrap();
    let mut retrieved = String::new();
    let mut next = Some(0);
    while let Some(offset) = next {
        let page = session
            .read_artifact(&artifact.id, ArtifactRange::new(offset, 4096).unwrap())
            .unwrap();
        assert!(page.content.len() <= 4096);
        next = page.next_offset;
        retrieved.push_str(&page.content);
    }
    assert_eq!(retrieved, content);
    assert!(ArtifactRange::new(0, 4097).is_err());
    assert!(
        session
            .read_artifact(
                &artifact.id,
                ArtifactRange::new(content.len() + 1, 10).unwrap()
            )
            .is_err()
    );
    assert!(
        session
            .read_artifact(&artifact.id, ArtifactRange::new(8, 10).unwrap())
            .is_err()
    );
}

#[test]
fn artifact_and_completion_abort_together() {
    let workspace = Workspace::new();
    let store = workspace.store();
    let session = store
        .create(SessionProvider::Injected, None, history())
        .unwrap();
    let pending = batch();
    let operation = pending.operations[0].clone();
    session.record(Event::Prepared(pending)).unwrap();
    session
        .record(Event::Intent {
            operation: operation.id.clone(),
            effect: ToolEffect::Write,
        })
        .unwrap();
    let before = serde_json::to_value(session.snapshot().unwrap()).unwrap();
    let result = ToolResult {
        outcome: Ok("large output ".repeat(10_000)),
        ..success(&operation)
    };
    assert!(
        session
            .complete_tool("unknown operation".into(), result)
            .is_err()
    );
    assert_eq!(
        serde_json::to_value(session.snapshot().unwrap()).unwrap(),
        before
    );
    let transaction = store.env.read_txn().unwrap();
    assert_eq!(store.artifacts.len(&transaction).unwrap(), 0);
    assert!(
        store
            .artifact_index
            .list(&transaction, &session.id)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn original_session_snapshots_resume_and_archive_legacy_inline_outputs() {
    use super::artifacts::{ArtifactRange, INLINE_BYTES};
    let workspace = Workspace::new();
    let store = workspace.store();
    let session = store
        .create(SessionProvider::Injected, None, history())
        .unwrap();
    let mut pending = batch();
    pending.operations.truncate(1);
    pending.assistant.content.truncate(1);
    let operation = pending.operations[0].clone();
    let original = "legacy output ".repeat(10_000);
    session.record(Event::Prepared(pending)).unwrap();
    session
        .record(Event::Intent {
            operation: operation.id.clone(),
            effect: ToolEffect::Read,
        })
        .unwrap();
    session
        .record(Event::Completed {
            operation: operation.id.clone(),
            result: ToolResult {
                outcome: Ok(original.clone()),
                ..success(&operation)
            },
        })
        .unwrap();
    let mut saved = serde_json::to_value(session.snapshot().unwrap()).unwrap();
    for field in ["artifacts", "forked_from", "context", "questions"] {
        saved.as_object_mut().unwrap().remove(field);
    }
    let mut transaction = store.env.write_txn().unwrap();
    store
        .snapshots
        .put(
            &mut transaction,
            &session.id,
            &serde_json::to_vec(&saved).unwrap(),
        )
        .unwrap();
    transaction.commit().unwrap();
    let id = session.id.clone();
    drop(session);
    let resumed = workspace
        .resume(&store, &id, &SessionProvider::Injected)
        .unwrap();
    let snapshot = resumed.snapshot().unwrap();
    assert_eq!(snapshot.context.generation, 0);
    assert_eq!(snapshot.artifacts.len(), 1);
    assert!(snapshot.history.last().unwrap().to_string().len() < INLINE_BYTES);
    let page = resumed
        .read_artifact(
            &snapshot.artifacts[0].id,
            ArtifactRange::new(0, 4096).unwrap(),
        )
        .unwrap();
    assert_eq!(page.content, original[..4096]);
}

#[test]
fn worker_artifacts_are_accessible_to_parents_and_snapshot_into_forks() {
    use super::artifacts::ArtifactRange;
    let workspace = Workspace::new();
    let store = workspace.store();
    let parent = store
        .create(SessionProvider::Injected, None, history())
        .unwrap();
    let worker = store
        .create(
            SessionProvider::Injected,
            Some(parent.id.clone()),
            history(),
        )
        .unwrap();
    save_output(&worker, &"worker output".repeat(1000));
    let artifact = worker.snapshot().unwrap().artifacts[0].id.clone();
    let nested = store
        .create(
            SessionProvider::Injected,
            Some(worker.id.clone()),
            history(),
        )
        .unwrap();
    save_output(&nested, &"nested output".repeat(1000));
    let nested_artifact = nested
        .snapshot()
        .unwrap()
        .artifacts
        .last()
        .unwrap()
        .id
        .clone();
    let fork = parent.fork().unwrap();
    for session in [&parent, &worker, &fork] {
        assert!(
            session
                .read_artifact(&nested_artifact, ArtifactRange::new(0, 1024).unwrap())
                .is_ok()
        );
    }
    assert!(
        parent
            .read_artifact(&artifact, ArtifactRange::new(0, 1024).unwrap())
            .is_ok()
    );
    assert!(
        fork.read_artifact(&artifact, ArtifactRange::new(0, 1024).unwrap())
            .is_ok()
    );
    for session in [&worker, &nested] {
        save_output(session, &"future output".repeat(1000));
        let later = session
            .snapshot()
            .unwrap()
            .artifacts
            .last()
            .unwrap()
            .id
            .clone();
        assert!(
            parent
                .read_artifact(&later, ArtifactRange::new(0, 1024).unwrap())
                .is_ok()
        );
        assert!(
            fork.read_artifact(&later, ArtifactRange::new(0, 1024).unwrap())
                .is_err()
        );
    }
}

#[test]
fn artifact_reads_and_forks_use_the_lmdb_index() {
    use super::artifacts::ArtifactRange;
    let workspace = Workspace::new();
    let store = workspace.store();
    let parent = store
        .create(SessionProvider::Injected, None, history())
        .unwrap();
    let worker = store
        .create(
            SessionProvider::Injected,
            Some(parent.id.clone()),
            history(),
        )
        .unwrap();
    let unrelated = store
        .create(SessionProvider::Injected, None, history())
        .unwrap();
    let content = "indexed worker output".repeat(1000);
    save_output(&worker, &content);
    let artifact = worker.snapshot().unwrap().artifacts[0].clone();
    invalidate(&store, &worker.id);
    invalidate(&store, &unrelated.id);
    let page = parent
        .read_artifact(&artifact.id, ArtifactRange::new(0, 4096).unwrap())
        .unwrap();
    assert_eq!(page.content, content[..4096]);
    let fork = parent.fork().unwrap();
    assert_eq!(fork.snapshot().unwrap().artifacts.len(), 1);
    assert_eq!(
        fork.read_artifact(&artifact.id, ArtifactRange::new(0, 4096).unwrap())
            .unwrap()
            .content,
        page.content
    );
    let next_worker = store
        .create(
            SessionProvider::Injected,
            Some(parent.id.clone()),
            history(),
        )
        .unwrap();
    assert_eq!(next_worker.snapshot().unwrap().artifacts[0].id, artifact.id);
    invalidate(&store, &parent.id);
    assert_eq!(
        parent
            .read_artifact(&artifact.id, ArtifactRange::new(0, 4096).unwrap())
            .unwrap()
            .content,
        page.content
    );
}

#[test]
fn existing_stores_build_the_artifact_index_once_and_preserve_fork_boundaries() {
    use super::artifacts::{ArtifactRange, ArtifactReference};
    let workspace = Workspace::new();
    let policy = WorkspacePolicy::workspace(workspace.path.clone()).unwrap();
    let storage = policy.session_storage("sessions").unwrap();
    let env = unsafe {
        EnvOpenOptions::new()
            .map_size(1024 * 1024 * 1024)
            .max_dbs(5)
            .open(storage.path())
            .unwrap()
    };
    let mut transaction = env.write_txn().unwrap();
    let snapshots: Database<Str, Bytes> = env
        .create_database(&mut transaction, Some("session_snapshots"))
        .unwrap();
    let _: Database<Str, Bytes> = env
        .create_database(&mut transaction, Some("session_events"))
        .unwrap();
    let _: Database<Str, Bytes> = env
        .create_database(&mut transaction, Some("session_owners"))
        .unwrap();
    let artifacts: Database<Str, Bytes> = env
        .create_database(&mut transaction, Some("session_artifacts"))
        .unwrap();
    let first = ArtifactReference {
        id: "first".into(),
        bytes: 5,
    };
    let later = ArtifactReference {
        id: "later".into(),
        bytes: 5,
    };
    artifacts
        .put(&mut transaction, &first.id, b"first")
        .unwrap();
    artifacts
        .put(&mut transaction, &later.id, b"later")
        .unwrap();
    let record = |id: &str, parent: Option<&str>, artifacts: Vec<ArtifactReference>| {
        serde_json::json!({
            "version": VERSION,
            "sequence": 1,
            "id": id,
            "workspace": storage.workspace_identity(),
            "provider": SessionProvider::Injected,
            "parent": parent,
            "history": history(),
            "pending": null,
            "queued": [],
            "status": Lifecycle::Ready,
            "usage": TokenCount::default(),
            "artifacts": artifacts
        })
    };
    for snapshot in [
        record("root", None, vec![]),
        record("worker", Some("root"), vec![first.clone()]),
        record("nested", Some("worker"), vec![first.clone(), later.clone()]),
        record("fork", None, vec![first.clone()]),
    ] {
        snapshots
            .put(
                &mut transaction,
                snapshot["id"].as_str().unwrap(),
                &serde_json::to_vec(&snapshot).unwrap(),
            )
            .unwrap();
    }
    transaction.commit().unwrap();
    drop(env);
    let store = workspace.store();
    let root = workspace
        .resume(&store, "root", &SessionProvider::Injected)
        .unwrap();
    let fork = workspace
        .resume(&store, "fork", &SessionProvider::Injected)
        .unwrap();
    assert_eq!(
        root.read_artifact(&later.id, ArtifactRange::new(0, 5).unwrap())
            .unwrap()
            .content,
        "later"
    );
    assert_eq!(
        fork.read_artifact(&first.id, ArtifactRange::new(0, 5).unwrap())
            .unwrap()
            .content,
        "first"
    );
    assert!(
        fork.read_artifact(&later.id, ArtifactRange::new(0, 5).unwrap())
            .is_err()
    );
    invalidate(&store, "worker");
    drop(fork);
    drop(root);
    drop(store);
    let store = workspace.store();
    let transaction = store.env.read_txn().unwrap();
    assert_eq!(
        store
            .artifact_index
            .list(&transaction, "root")
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        store
            .artifact_index
            .list(&transaction, "fork")
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn forkable_snapshot_requires_idle_status_without_pending_or_queued_work() {
    let workspace = Workspace::new();
    let store = workspace.store();
    let session = store
        .create(SessionProvider::Injected, None, history())
        .unwrap();
    let snapshot = session.snapshot().unwrap();

    for (status, idle) in [
        (Lifecycle::Ready, true),
        (Lifecycle::Running, false),
        (Lifecycle::WaitingForTools, false),
        (Lifecycle::WaitingForInput, true),
        (Lifecycle::Cancelling, false),
        (Lifecycle::Completed, true),
        (Lifecycle::Cancelled, true),
        (Lifecycle::Failed, true),
    ] {
        for pending in [false, true] {
            for queued in [false, true] {
                let mut candidate = snapshot.clone();
                candidate.status = status;
                candidate.pending = pending.then(batch);
                if queued {
                    candidate.queued.push(QueuedInput {
                        turn: "queued-turn".into(),
                        prompt: Some("next input".into()),
                    });
                }
                let result = ForkableSnapshot::try_from(candidate.clone());
                assert_eq!(
                    result.is_ok(),
                    idle && !pending && !queued,
                    "status={status:?}, pending={pending}, queued={queued}"
                );
                match result {
                    Ok(ForkableSnapshot(validated)) => assert_eq!(
                        serde_json::to_value(validated).unwrap(),
                        serde_json::to_value(candidate).unwrap()
                    ),
                    Err(error) => assert_eq!(
                        error.to_string(),
                        "Fork requires an idle session with no pending operations"
                    ),
                }
            }
        }
    }
}

#[test]
fn compacted_sessions_reject_replayed_checkpoints_and_isolate_fork_questions() {
    use crate::context::{Checkpoint, Memory};
    let workspace = Workspace::new();
    let store = workspace.store();
    let session = store
        .create(SessionProvider::Injected, None, history())
        .unwrap();
    session
        .record(Event::History(vec![Message::new_assistant(
            "old response".into(),
        )]))
        .unwrap();
    let question = PendingQuestion {
        id: "q1".to_owned().try_into().unwrap(),
        prompt: "Which target?".to_owned().try_into().unwrap(),
        required: true,
    };
    session
        .record(Event::QuestionAsked(question.clone()))
        .unwrap();
    assert!(session.record(Event::QuestionAsked(question)).is_err());
    let original = session.snapshot().unwrap().history;
    let checkpoint = Checkpoint::new(
        &original,
        original.len(),
        1,
        Memory::Summary("Implementation remains pending".into()),
    )
    .unwrap();
    session
        .record(Event::Compacted {
            context: checkpoint.clone(),
            usage: TokenCount::default(),
        })
        .unwrap();
    assert!(
        session
            .record(Event::Compacted {
                context: checkpoint,
                usage: TokenCount::default()
            })
            .is_err()
    );
    let id = session.id.clone();
    drop(session);
    let session = workspace
        .resume(&store, &id, &SessionProvider::Injected)
        .unwrap();
    let fork = session.fork().unwrap();
    assert_eq!(
        fork.snapshot().unwrap().forked_from.as_deref(),
        Some(session.id.as_str())
    );
    fork.record(Event::QuestionAnswered {
        id: "q1".into(),
        answer: "binary".into(),
    })
    .unwrap();
    assert!(fork.snapshot().unwrap().questions.is_empty());
    assert_eq!(session.snapshot().unwrap().questions.len(), 1);
    assert_eq!(
        serde_json::to_value(session.snapshot().unwrap().history).unwrap(),
        serde_json::to_value(original).unwrap()
    );
}

fn batch() -> PendingBatch {
    let operations: Vec<_> = ["done", "uncertain", "unstarted"]
        .into_iter()
        .map(|id| Operation {
            id: id.into(),
            call: crate::tool_call::ToolCall {
                id: ToolId {
                    id: id.to_owned().try_into().unwrap(),
                    call_id: Some(format!("call_{id}").try_into().unwrap()),
                },
                name: "write".to_owned().try_into().unwrap(),
                input: Default::default(),
            },
            state: OperationState::Queued,
        })
        .collect();
    let assistant = Message {
        role: Role::Assistant,
        content: operations
            .iter()
            .map(|operation| operation.call.content())
            .collect(),
    };
    PendingBatch {
        assistant,
        operations,
    }
}

fn success(operation: &Operation) -> ToolResult {
    ToolResult {
        id: operation.call.id.clone(),
        invocation: ToolInvocation {
            name: operation.call.name.clone(),
            input: operation.call.input.clone(),
            display: "write".into(),
        },
        outcome: Ok("completed output".into()),
    }
}

#[test]
fn atomic_events_and_snapshots_survive_reopen_with_native_content() {
    let workspace = Workspace::new();
    let store = workspace.store();
    let session = store
        .create(SessionProvider::Injected, None, history())
        .unwrap();
    let message = Message {
        role: Role::Assistant,
        content: vec![
            ContentBlock::OpenAIReasoning(clients::openai::ReasoningItem {
                id: "reasoning-id".into(),
                summary: vec![],
                encrypted_content: Some("opaque ciphertext".into()),
                extra: serde_json::from_value(
                    serde_json::json!({"status": "completed", "future_field": "opaque"}),
                )
                .unwrap(),
            }),
            ContentBlock::ThinkingBlock {
                thinking: "thinking".into(),
                signature: "opaque signature".into(),
                reasoning_id: None,
            },
            ContentBlock::MessageBlock {
                text: "done".into(),
                phase: Some(clients::llm::MessagePhase::FinalAnswer),
            },
        ],
    };
    session
        .record(Event::History(vec![message.clone()]))
        .unwrap();
    session
        .record(Event::Usage(TokenCount {
            input_tokens: 120,
            output_tokens: 35,
        }))
        .unwrap();
    let id = session.id.clone();
    drop(session);
    drop(store);
    let store = workspace.store();
    let session = workspace
        .resume(&store, &id, &SessionProvider::Injected)
        .unwrap();
    let snapshot = session.snapshot().unwrap();
    assert_eq!(
        serde_json::to_value(&snapshot.history[2]).unwrap(),
        serde_json::to_value(message).unwrap()
    );
    assert_eq!(snapshot.usage.input_tokens, 120);
    assert_eq!(snapshot.usage.output_tokens, 35);
    let transaction = store.env.read_txn().unwrap();
    let records = store
        .events
        .prefix_iter(&transaction, &format!("{id}:"))
        .unwrap()
        .map(|entry| decode::<Record>(entry.unwrap().1).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(records.len() as u64, snapshot.sequence);
    assert_eq!(
        records
            .iter()
            .map(|record| record.sequence)
            .collect::<Vec<_>>(),
        (1..=snapshot.sequence).collect::<Vec<_>>()
    );
}

#[test]
fn resume_choices_use_recency_and_exclude_workers_empty_current_and_other_providers() {
    let workspace = Workspace::new();
    let store = workspace.store();
    let older = store
        .create(SessionProvider::Injected, None, history())
        .unwrap();
    let newer = store
        .create(
            SessionProvider::Injected,
            None,
            vec![
                Message::new("context".into()),
                Message::new("Fix résumé search".into()),
            ],
        )
        .unwrap();
    let current = store
        .create(SessionProvider::Injected, None, history())
        .unwrap();
    let _worker = store
        .create(
            SessionProvider::Injected,
            Some(current.id.clone()),
            history(),
        )
        .unwrap();
    let _other = store
        .create(SessionProvider::Claude, None, history())
        .unwrap();
    let _empty = store
        .create(
            SessionProvider::Injected,
            None,
            vec![Message::new("context".into())],
        )
        .unwrap();
    let mut saved = serde_json::to_value(older.snapshot().unwrap()).unwrap();
    saved.as_object_mut().unwrap().remove("updated_at");
    let mut transaction = store.env.write_txn().unwrap();
    store
        .snapshots
        .put(
            &mut transaction,
            &older.id,
            &serde_json::to_vec(&saved).unwrap(),
        )
        .unwrap();
    transaction.commit().unwrap();
    let choices = store
        .resume_choices(&SessionProvider::Injected, Some(&current.id))
        .unwrap();
    assert_eq!(choices.len(), 2);
    assert_eq!(choices[0].id, newer.id);
    assert_eq!(choices[0].title, "Fix résumé search");
    assert!(choices[0].updated_at.is_some());
    assert_eq!(choices[1].id, older.id);
    assert!(choices[1].updated_at.is_none());
}

#[test]
fn recovery_pairs_all_calls_and_never_reexecutes_uncertain_operations() {
    let workspace = Workspace::new();
    let store = workspace.store();
    let session = store
        .create(SessionProvider::Injected, None, history())
        .unwrap();
    let batch = batch();
    let result = success(&batch.operations[0]);
    session.record(Event::Prepared(batch)).unwrap();
    for operation in ["done", "uncertain"] {
        session
            .record(Event::Intent {
                operation: operation.into(),
                effect: ToolEffect::Write,
            })
            .unwrap();
    }
    session
        .record(Event::Completed {
            operation: "done".into(),
            result,
        })
        .unwrap();
    session
        .record(Event::Queued(QueuedInput {
            turn: "follow-up".into(),
            prompt: Some("Also preserve API compatibility".into()),
        }))
        .unwrap();
    let id = session.id.clone();
    drop(session);
    let session = workspace
        .resume(&store, &id, &SessionProvider::Injected)
        .unwrap();
    let snapshot = session.snapshot().unwrap();
    assert!(snapshot.pending.is_none());
    assert!(snapshot.queued.is_empty());
    let calls = &snapshot.history[2].content;
    let results = &snapshot.history[3].content;
    assert_eq!(calls.len(), 3);
    assert_eq!(results.len(), 3);
    for (call, result) in calls.iter().zip(results) {
        assert!(
            matches!((call, result), (ContentBlock::ToolBlock {tool_id: call, ..}, ContentBlock::ToolResult {tool_id: result, ..}) if call == result)
        );
    }
    assert!(
        matches!(&results[0], ContentBlock::ToolResult { content, is_error: None, .. } if content == "completed output")
    );
    assert!(
        matches!(&results[1], ContentBlock::ToolResult { content, is_error: Some(true), .. } if content.contains("Uncertain operation"))
    );
    assert!(
        matches!(&results[2], ContentBlock::ToolResult { content, is_error: Some(true), .. } if content.contains("Not executed"))
    );
    assert_eq!(
        snapshot.history[4].text(),
        "Also preserve API compatibility"
    );
    let expected = serde_json::to_value(&snapshot.history).unwrap();
    drop(session);
    let session = workspace
        .resume(&store, &id, &SessionProvider::Injected)
        .unwrap();
    assert_eq!(
        serde_json::to_value(session.snapshot().unwrap().history).unwrap(),
        expected
    );
}

#[test]
fn invalid_operation_transitions_and_failed_transactions_preserve_the_committed_state() {
    let workspace = Workspace::new();
    let store = workspace.store();
    let session = store
        .create(SessionProvider::Injected, None, history())
        .unwrap();
    let batch = batch();
    let result = success(&batch.operations[0]);
    session.record(Event::Prepared(batch)).unwrap();
    assert!(
        session
            .record(Event::Completed {
                operation: "done".into(),
                result: result.clone()
            })
            .is_err()
    );
    assert_eq!(session.snapshot().unwrap().sequence, 2);
    session
        .record(Event::Intent {
            operation: "done".into(),
            effect: ToolEffect::Write,
        })
        .unwrap();
    let sequence = session.snapshot().unwrap().sequence;
    assert!(
        session
            .record(Event::Intent {
                operation: "done".into(),
                effect: ToolEffect::Write
            })
            .is_err()
    );
    assert_eq!(session.snapshot().unwrap().sequence, sequence);
    let mut mismatched = result.clone();
    mismatched.invocation.name = "different-tool".to_owned().try_into().unwrap();
    assert!(
        session
            .record(Event::Completed {
                operation: "done".into(),
                result: mismatched
            })
            .is_err()
    );
    let mut mismatched = result.clone();
    mismatched
        .invocation
        .input
        .insert("unexpected".into(), serde_json::json!(true));
    assert!(
        session
            .record(Event::Completed {
                operation: "done".into(),
                result: mismatched
            })
            .is_err()
    );
    assert_eq!(session.snapshot().unwrap().sequence, sequence);
    session
        .record(Event::Completed {
            operation: "done".into(),
            result: result.clone(),
        })
        .unwrap();
    let sequence = session.snapshot().unwrap().sequence;
    assert!(
        session
            .record(Event::Completed {
                operation: "done".into(),
                result
            })
            .is_err()
    );
    assert_eq!(session.snapshot().unwrap().sequence, sequence);
    let mut transaction = store.env.write_txn().unwrap();
    let mut snapshot = session.snapshot().unwrap();
    snapshot.history.clear();
    snapshot.sequence += 1;
    store
        .write(&mut transaction, &snapshot, Event::Recovered)
        .unwrap();
    transaction.abort();
    assert_eq!(session.snapshot().unwrap().sequence, sequence);
    assert_eq!(session.snapshot().unwrap().history.len(), 2);
}

#[test]
fn map_exhaustion_keeps_the_last_snapshot_and_event_sequence() {
    let workspace = Workspace::new();
    let store = workspace.store();
    let session = store
        .create(SessionProvider::Injected, None, history())
        .unwrap();
    unsafe { store.env.resize(64 * 1024).unwrap() };
    let result = session.record(Event::History(vec![Message::new("x".repeat(128 * 1024))]));
    assert!(result.is_err());
    assert_eq!(session.snapshot().unwrap().sequence, 1);
    assert_eq!(session.snapshot().unwrap().history.len(), 2);
    assert!(
        store
            .create(
                SessionProvider::Injected,
                None,
                vec![Message::new("x".repeat(128 * 1024))],
            )
            .is_err()
    );
    let transaction = store.env.read_txn().unwrap();
    assert_eq!(store.events.len(&transaction).unwrap(), 1);
    assert_eq!(store.snapshots.len(&transaction).unwrap(), 1);
    assert_eq!(store.owners.len(&transaction).unwrap(), 1);
}

#[test]
fn another_process_cannot_take_ownership_of_a_loaded_session() {
    let workspace = Workspace::new();
    let store = workspace.store();
    let session = store
        .create(SessionProvider::Injected, None, history())
        .unwrap();
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "session::tests::lock_fixture", "--nocapture"])
        .env("JOE_M4_LOCK_WORKSPACE", &workspace.path)
        .env("JOE_M4_LOCK_SESSION", &session.id)
        .status()
        .unwrap();
    assert!(status.success());
    session
        .record(Event::History(vec![Message::new_assistant(
            "still owned".into(),
        )]))
        .unwrap();
}

#[test]
fn lock_fixture() {
    if let Some(path) = std::env::var_os("JOE_M4_LOCK_WORKSPACE") {
        let path = PathBuf::from(path);
        let store = open(&path);
        let policy = WorkspacePolicy::workspace(path).unwrap();
        let id = std::env::var("JOE_M4_LOCK_SESSION").unwrap();
        assert!(
            ResumableSession::new(&store, &id, &policy, &SessionProvider::Injected)
                .err()
                .unwrap()
                .to_string()
                .contains("already open")
        );
    }
}

enum ClaimState {
    Pending,
    Acquired,
    Denied,
}

struct ClaimProcess {
    process: std::process::Child,
    output: std::io::BufReader<std::process::ChildStdout>,
    state: ClaimState,
}

impl ClaimProcess {
    fn new(workspace: &Workspace, id: &str) -> Self {
        let mut process = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "session::tests::claim_fixture", "--nocapture"])
            .env("JOE_M4_CLAIM_WORKSPACE", &workspace.path)
            .env("JOE_M4_CLAIM_SESSION", id)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let output = std::io::BufReader::new(process.stdout.take().unwrap());
        let mut claim = Self {
            process,
            output,
            state: ClaimState::Pending,
        };
        assert_eq!(claim.next_message(), "claim:ready");
        claim
    }

    fn signal(&mut self) {
        use std::io::Write;
        self.process
            .stdin
            .as_mut()
            .unwrap()
            .write_all(b"x")
            .unwrap();
    }

    fn next_message(&mut self) -> String {
        use std::io::{BufRead, Read};
        self.output
            .by_ref()
            .lines()
            .map(Result::unwrap)
            .find(|line| line.starts_with("claim:"))
            .unwrap()
    }

    fn receive(&mut self) {
        let line = self.next_message();
        self.state = match line.as_str() {
            "claim:acquired" => ClaimState::Acquired,
            "claim:denied" => ClaimState::Denied,
            _ => panic!("Unexpected claim outcome: {line}"),
        };
    }

    fn finish(&mut self) {
        match self.state {
            ClaimState::Acquired => self.signal(),
            ClaimState::Denied => {}
            ClaimState::Pending => panic!("Claim outcome is still pending"),
        }
        assert!(self.process.wait().unwrap().success());
    }
}

impl Drop for ClaimProcess {
    fn drop(&mut self) {
        let _ = self.process.kill();
        let _ = self.process.wait();
    }
}

#[test]
fn competing_processes_claim_one_lmdb_owner_and_release_it_on_exit() {
    let workspace = Workspace::new();
    let store = workspace.store();
    let session = store
        .create(SessionProvider::Injected, None, history())
        .unwrap();
    let id = session.id.clone();
    drop(session);
    let mut contenders = [
        ClaimProcess::new(&workspace, &id),
        ClaimProcess::new(&workspace, &id),
    ];
    contenders.iter_mut().for_each(ClaimProcess::signal);
    contenders.iter_mut().for_each(ClaimProcess::receive);
    assert_eq!(
        contenders
            .iter()
            .filter(|claim| matches!(claim.state, ClaimState::Acquired))
            .count(),
        1
    );
    let transaction = store.env.read_txn().unwrap();
    assert!(store.owner(&transaction, &id).unwrap().is_some());
    drop(transaction);
    assert!(
        workspace
            .resume(&store, &id, &SessionProvider::Injected)
            .is_err()
    );
    contenders.iter_mut().for_each(ClaimProcess::finish);
    let session = workspace
        .resume(&store, &id, &SessionProvider::Injected)
        .unwrap();
    assert_eq!(session.snapshot().unwrap().sequence, 2);
    assert!(
        !store
            .storage
            .path()
            .join(format!("{id}.session-lock"))
            .exists()
    );
}

#[test]
fn claim_fixture() {
    use std::io::{Read, Write};
    if let Some(path) = std::env::var_os("JOE_M4_CLAIM_WORKSPACE") {
        let path = PathBuf::from(path);
        let store = open(&path);
        let policy = WorkspacePolicy::workspace(path).unwrap();
        let id = std::env::var("JOE_M4_CLAIM_SESSION").unwrap();
        println!("claim:ready");
        std::io::stdout().flush().unwrap();
        std::io::stdin().read_exact(&mut [0]).unwrap();
        match ResumableSession::new(&store, &id, &policy, &SessionProvider::Injected) {
            Ok(session) => {
                println!("claim:acquired");
                std::io::stdout().flush().unwrap();
                std::io::stdin().read_exact(&mut [0]).unwrap();
                drop(session);
            }
            Err(error) => {
                assert!(error.to_string().contains("already open"));
                println!("claim:denied");
                std::io::stdout().flush().unwrap();
            }
        }
    }
}

#[test]
fn stale_handles_cannot_read_write_or_release_a_replacement_owner() {
    let workspace = Workspace::new();
    let store = workspace.store();
    let session = store
        .create(SessionProvider::Injected, None, history())
        .unwrap();
    save_output(&session, &"owned artifact".repeat(1000));
    let snapshot = session.snapshot().unwrap();
    let artifact = snapshot.artifacts[0].id.clone();
    let id = session.id.clone();
    let replacement = Owner::new(None, store.storage.new_id()).unwrap();
    let mut transaction = store.env.write_txn().unwrap();
    store
        .owners
        .put(
            &mut transaction,
            &id,
            &serde_json::to_vec(&replacement).unwrap(),
        )
        .unwrap();
    transaction.commit().unwrap();
    assert!(session.snapshot().is_err());
    assert!(
        session
            .read_artifact(&artifact, artifacts::ArtifactRange::new(0, 4096).unwrap())
            .is_err()
    );
    assert!(
        session
            .record(Event::History(vec![Message::new("stale write".into())]))
            .is_err()
    );
    drop(session);
    let transaction = store.env.read_txn().unwrap();
    assert!(store.owner(&transaction, &id).unwrap() == Some(replacement.clone()));
    assert_eq!(
        store.snapshot(&transaction, &id).unwrap().sequence,
        snapshot.sequence
    );
    drop(transaction);
    let current = Session {
        store: store.clone(),
        id: id.clone(),
        owner: replacement,
    };
    assert!(
        current
            .read_artifact(&artifact, artifacts::ArtifactRange::new(0, 4096).unwrap())
            .is_ok()
    );
    current
        .record(Event::History(vec![Message::new("current owner".into())]))
        .unwrap();
    drop(current);
    assert!(
        workspace
            .resume(&store, &id, &SessionProvider::Injected)
            .is_ok()
    );
}

#[test]
fn ownership_schema_provider_and_workspace_are_revalidated() {
    let workspace = Workspace::new();
    let store = workspace.store();
    let session = store
        .create(SessionProvider::Injected, None, history())
        .unwrap();
    let id = session.id.clone();
    assert!(
        workspace
            .resume(&store, &id, &SessionProvider::Injected)
            .err()
            .unwrap()
            .to_string()
            .contains("already open")
    );
    let held = session.clone();
    drop(session);
    assert!(
        workspace
            .resume(&store, &id, &SessionProvider::Injected)
            .is_err()
    );
    drop(held);
    let other_workspace = Workspace::new();
    assert!(
        other_workspace
            .resume(&store, &id, &SessionProvider::Injected)
            .err()
            .unwrap()
            .to_string()
            .contains("does not belong to the current workspace")
    );
    let policy = WorkspacePolicy::workspace(workspace.path.clone()).unwrap();
    let resumable =
        ResumableSession::new(&store, &id, &policy, &SessionProvider::Injected).unwrap();
    assert_eq!(store.list().unwrap()[0].sequence, 1);
    assert!(
        workspace
            .resume(&store, &id, &SessionProvider::Injected)
            .err()
            .unwrap()
            .to_string()
            .contains("already open")
    );
    drop(resumable);
    assert!(
        workspace
            .resume(&store, &id, &SessionProvider::Claude)
            .err()
            .unwrap()
            .to_string()
            .contains("provider")
    );
    let session = workspace
        .resume(&store, &id, &SessionProvider::Injected)
        .unwrap();
    let mut snapshot = session.snapshot().unwrap();
    assert_eq!(snapshot.sequence, 2);
    drop(session);
    snapshot.workspace = "different project".into();
    let mut transaction = store.env.write_txn().unwrap();
    store
        .snapshots
        .put(
            &mut transaction,
            &id,
            &serde_json::to_vec(&snapshot).unwrap(),
        )
        .unwrap();
    transaction.commit().unwrap();
    assert!(
        workspace
            .resume(&store, &id, &SessionProvider::Injected)
            .err()
            .unwrap()
            .to_string()
            .contains("workspace identity")
    );
    let mut transaction = store.env.write_txn().unwrap();
    store
        .snapshots
        .put(&mut transaction, &id, br#"{"version":999}"#)
        .unwrap();
    transaction.commit().unwrap();
    assert!(
        workspace
            .resume(&store, &id, &SessionProvider::Injected)
            .err()
            .unwrap()
            .to_string()
            .contains("Unsupported session schema version 999")
    );
    assert!(
        workspace
            .resume(&store, "../../outside", &SessionProvider::Injected)
            .is_err()
    );
}

#[test]
fn worker_linkage_is_retained_and_workers_cannot_be_resumed_as_roots() {
    let workspace = Workspace::new();
    let store = workspace.store();
    let root = store
        .create(SessionProvider::Injected, None, history())
        .unwrap();
    let child = store
        .create(SessionProvider::Injected, Some(root.id.clone()), history())
        .unwrap();
    let child_id = child.id.clone();
    assert_eq!(
        child.snapshot().unwrap().parent.as_deref(),
        Some(root.id.as_str())
    );
    drop(child);
    assert!(
        workspace
            .resume(&store, &child_id, &SessionProvider::Injected)
            .err()
            .unwrap()
            .to_string()
            .contains("parent session")
    );
}

#[test]
fn process_exit_leaves_a_durable_intent_and_discards_an_uncommitted_completion() {
    let workspace = Workspace::new();
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "session::tests::crash_fixture", "--nocapture"])
        .env("JOE_M4_CRASH_WORKSPACE", &workspace.path)
        .status()
        .unwrap();
    assert!(status.success());
    let store = workspace.store();
    let snapshots = store.list().unwrap();
    assert_eq!(snapshots.len(), 1);
    let session = workspace
        .resume(&store, &snapshots[0].id, &SessionProvider::Injected)
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(workspace.path.join("written.txt")).unwrap(),
        "one write"
    );
    assert!(
        session
            .snapshot()
            .unwrap()
            .history
            .iter()
            .any(|message| message.to_string().contains("Uncertain operation"))
    );
}

#[test]
fn crash_fixture() {
    if let Some(path) = std::env::var_os("JOE_M4_CRASH_WORKSPACE") {
        let path = PathBuf::from(path);
        let store = open(&path);
        let session = store
            .create(SessionProvider::Injected, None, history())
            .unwrap();
        let batch = batch();
        let result = success(&batch.operations[0]);
        session.record(Event::Prepared(batch)).unwrap();
        session
            .record(Event::Intent {
                operation: "done".into(),
                effect: ToolEffect::Write,
            })
            .unwrap();
        std::fs::write(path.join("written.txt"), "one write").unwrap();
        let mut snapshot = session.snapshot().unwrap();
        let event = Event::Completed {
            operation: "done".into(),
            result,
        };
        snapshot = snapshot.transition(&event).unwrap();
        snapshot.sequence += 1;
        let mut transaction = store.env.write_txn().unwrap();
        store.write(&mut transaction, &snapshot, event).unwrap();
        std::process::exit(0);
    }
}
