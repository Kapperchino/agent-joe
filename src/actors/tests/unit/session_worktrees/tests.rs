use super::*;
use analysis::contexts::rust_context::RustContext;
use commands::command::{Answer, Command, PruneMode, QuestionAnswer, ResumeTarget};
use common_models::interaction::QuestionPurpose;

struct GitHarness {
    workspace: crate::session::tests::Workspace,
    repo: git2::Repository,
    actor: ActorRef<Message>,
    handle: tokio::task::JoinHandle<()>,
    store: Arc<crate::session::SessionStore>,
    requests: flume::Receiver<Request>,
    events: flume::Receiver<ActorToTui>,
}

impl GitHarness {
    fn commit_main(&self, text: &str) -> git2::Oid {
        std::fs::write(self.workspace.path.join("lib.rs"), text).unwrap();
        let mut index = self.repo.index().unwrap();
        index.add_path(std::path::Path::new("lib.rs")).unwrap();
        index.write().unwrap();
        let tree = self.repo.find_tree(index.write_tree().unwrap()).unwrap();
        let parent = self.repo.head().unwrap().peel_to_commit().unwrap();
        let signature = git2::Signature::now("Fixture", "fixture@example.com").unwrap();
        self.repo
            .commit(
                Some("HEAD"),
                &signature,
                &signature,
                "concurrent main edit",
                &tree,
                &[&parent],
            )
            .unwrap()
    }
    async fn new() -> Self {
        let workspace = crate::session::tests::Workspace::new();
        let repo = git2::Repository::init(&workspace.path).unwrap();
        repo.set_head("refs/heads/main").unwrap();
        std::fs::write(
            workspace.path.join("lib.rs"),
            "pub fn value() -> u32 { 1 }\n",
        )
        .unwrap();
        let base = {
            let mut index = repo.index().unwrap();
            index.add_path(std::path::Path::new("lib.rs")).unwrap();
            index.write().unwrap();
            index.write_tree().unwrap()
        };
        let signature = git2::Signature::now("Fixture", "fixture@example.com").unwrap();
        repo.commit(
            Some("HEAD"),
            &signature,
            &signature,
            "base",
            &repo.find_tree(base).unwrap(),
            &[],
        )
        .unwrap();
        let runtime = Runtime::for_workspace(workspace.path.clone()).unwrap();
        let store = runtime.sessions.clone().unwrap();
        let context = RustContext::new("Fixture instructions".into(), 0, workspace.path.clone())
            .await
            .unwrap();
        let (tx, requests) = flume::unbounded();
        let (tui_tx, events) = flume::unbounded();
        let (actor, handle) = Actor::spawn(
            None,
            WorkerAdapter::new(crate::workers::simple_worker::SimpleWorker::<RustContext>::new()),
            Dependency {
                context,
                runtime,
                client: llm::LLmClient::Injected(Arc::new(Provider(tx))),
                tools: vec![
                    tools::tool_defs::erased_tool::<
                        tools::apply_patch::ApplyPatch,
                        RustContext,
                        ActorContext<RustContext>,
                    >(),
                    tools::tool_defs::erased_tool::<
                        tools::read_file::ReadFile,
                        RustContext,
                        ActorContext<RustContext>,
                    >(),
                ],
                tui_tx,
                debug_mode: false,
            },
        )
        .await
        .unwrap();
        Self {
            workspace,
            repo,
            actor,
            handle,
            store,
            requests,
            events,
        }
    }

    fn snapshot(&self, id: &str) -> crate::session::Snapshot {
        self.store
            .list()
            .unwrap()
            .into_iter()
            .find(|snapshot| snapshot.id == id)
            .unwrap()
    }

    async fn event(&self, predicate: impl Fn(&ActorToTuiPacket) -> bool) -> ActorToTuiPacket {
        within(async {
            let mut found = None;
            while found.is_none() {
                let packet = self.events.recv_async().await.unwrap().packet;
                found = predicate(&packet).then_some(packet);
            }
            found.unwrap()
        })
        .await
    }

    async fn command(&self, command: Command) -> String {
        self.actor
            .send_message(Message::Command(command.clone()))
            .unwrap();
        let packet = self.event(|packet| matches!(packet, ActorToTuiPacket::CommandResult(actual, _) if actual == &command)).await;
        match packet {
            ActorToTuiPacket::CommandResult(_, message) => message,
            _ => panic!("Unexpected command response"),
        }
    }

    async fn complete(&self, patch: Option<&str>) -> common_models::interaction::Question {
        self.complete_with_subject(patch, "Make value return 2 instead of 1")
            .await
    }

    async fn summarize(&self, subject: &str) -> llm::ClientRequest {
        let (request, reply) = within(self.requests.recv_async()).await.unwrap();
        assert!(request.tools.is_empty());
        assert_eq!(request.messages.len(), 1);
        assert!(request.messages[0].text().contains("diff --git"));
        answer(reply, response(vec![text(subject)]));
        request
    }

    async fn complete_with_subject(
        &self,
        patch: Option<&str>,
        subject: &str,
    ) -> common_models::interaction::Question {
        self.actor
            .send_message(Message::StartWork(Some("Update the function".into())))
            .unwrap();
        let (_, reply) = within(self.requests.recv_async()).await.unwrap();
        let reply = match patch {
            Some(patch) => {
                let mut block = call("apply_patch", "edit");
                if let ContentBlock::ToolBlock { input, .. } = &mut block {
                    *input = json!({ "patch": patch }).as_object().unwrap().clone();
                }
                answer(reply, response(vec![block]));
                let (request, reply) = within(self.requests.recv_async()).await.unwrap();
                assert!(
                    request
                        .messages
                        .iter()
                        .any(|message| message.content.iter().any(|content| matches!(
                            content,
                            ContentBlock::ToolResult {
                                is_error: None | Some(false),
                                ..
                            }
                        )))
                );
                reply
            }
            None => reply,
        };
        answer(reply, response(vec![text("Task completed")]));
        self.summarize(subject).await;
        let packet = self.event(|packet| matches!(packet, ActorToTuiPacket::InteractionUpdated(view) if view.questions.iter().any(|question| question.id.starts_with("merge-")))).await;
        match packet {
            ActorToTuiPacket::InteractionUpdated(view) => view
                .questions
                .into_iter()
                .find(|question| question.id.starts_with("merge-"))
                .unwrap(),
            _ => panic!("Unexpected merge response"),
        }
    }

    async fn answer_merge(
        &self,
        question: &common_models::interaction::Question,
        choice: &str,
    ) -> String {
        self.command(Command::Answer(QuestionAnswer {
            id: question.id.clone(),
            answer: Answer::Choice {
                choice_id: choice.into(),
            },
        }))
        .await
    }

    async fn stop(self) {
        self.actor.send_message(Message::KYS).unwrap();
        within(self.handle).await.unwrap();
    }
}

const PATCH: &str = "*** Begin Patch\n*** Update File: lib.rs\n@@\n-pub fn value() -> u32 { 1 }\n+pub fn value() -> u32 { 2 }\n*** End Patch";

#[tokio::test]
async fn prune_removes_inactive_merged_worktrees() {
    let h = GitHarness::new().await;
    let id = h.store.list().unwrap()[0].id.clone();
    let worktree = h.snapshot(&id).worktree.unwrap();
    h.command(Command::New).await;
    let message = h.command(Command::parse("prune").unwrap()).await;
    assert!(
        message.contains("Pruned 1 session worktree(s); skipped 1"),
        "{message}"
    );
    assert!(!worktree.path.exists());
    assert!(h.repo.find_worktree(&id).is_err());
    assert!(h.snapshot(&id).worktree.is_none());
    h.stop().await;
}

#[tokio::test]
async fn force_prune_discards_inactive_worktrees_preserves_history_and_allows_resume() {
    let h = GitHarness::new().await;
    let id = h.store.list().unwrap()[0].id.clone();
    let worktree = h.snapshot(&id).worktree.unwrap();
    let question = h.complete(Some(PATCH)).await;
    let saved = h.snapshot(&id);
    h.command(Command::New).await;
    let current = h
        .store
        .list()
        .unwrap()
        .into_iter()
        .find(|snapshot| snapshot.id != id)
        .unwrap();
    let current_worktree = current.worktree.unwrap();
    let main = h.repo.refname_to_id("HEAD").unwrap();
    let index = std::fs::read(h.repo.path().join("index")).unwrap();
    let message = h.command(Command::Prune(PruneMode::Merged)).await;
    assert!(message.contains("Pruned 0"), "{message}");
    assert!(message.contains("use /prune --force"), "{message}");
    assert!(worktree.path.exists());
    assert!(h.snapshot(&id).worktree.is_some());
    assert_eq!(
        serde_json::to_value(&h.snapshot(&id).history).unwrap(),
        serde_json::to_value(&saved.history).unwrap()
    );
    let message = h.command(Command::parse("prune --force").unwrap()).await;
    assert!(
        message.contains("Pruned 1 session worktree(s)"),
        "{message}"
    );
    assert!(
        message.contains(&format!("Skipped {}", current.id)),
        "{message}"
    );
    assert!(!worktree.path.exists());
    assert!(h.repo.find_worktree(&id).is_err());
    assert!(
        h.repo
            .find_branch(&format!("joe/session/{id}"), git2::BranchType::Local)
            .is_err()
    );
    assert!(current_worktree.path.exists());
    assert_eq!(h.repo.refname_to_id("HEAD").unwrap(), main);
    assert_eq!(std::fs::read(h.repo.path().join("index")).unwrap(), index);
    let pruned = h.snapshot(&id);
    assert!(pruned.worktree.is_none());
    assert!(pruned.questions.pending().is_empty());
    assert!(matches!(
        pruned.merge_approval,
        crate::session::session_merge::MergeApproval::None
    ));
    assert_eq!(
        serde_json::to_value(&pruned.history[..saved.history.len()]).unwrap(),
        serde_json::to_value(&saved.history).unwrap()
    );
    assert!(
        pruned
            .history
            .last()
            .unwrap()
            .text()
            .contains("worktree was pruned")
    );
    assert!(
        h.command(Command::Prune(PruneMode::Force))
            .await
            .contains("Pruned 0")
    );
    let later_main = h.commit_main("pub fn value() -> u32 { 8 }\n");
    h.actor
        .send_message(Message::Command(Command::Resume(ResumeTarget::Session {
            id: id.clone(),
        })))
        .unwrap();
    assert!(matches!(
        h.event(|packet| matches!(packet, ActorToTuiPacket::SessionResumed(_)))
            .await,
        ActorToTuiPacket::SessionResumed(Ok(_))
    ));
    assert_eq!(
        git2::Repository::open(&worktree.path)
            .unwrap()
            .refname_to_id("HEAD")
            .unwrap(),
        later_main
    );
    h.answer_merge(&question, "merge").await;
    assert_eq!(h.repo.refname_to_id("HEAD").unwrap(), later_main);
    assert!(worktree.path.exists());
    h.stop().await;
}

#[tokio::test]
async fn prune_skips_live_sessions_and_continues_after_locked_worktrees() {
    let h = GitHarness::new().await;
    let project = utils::workspace::WorkspacePolicy::workspace(h.workspace.path.clone()).unwrap();
    let live = h
        .store
        .create(llm::SessionProvider::Injected, None, Vec::new())
        .unwrap();
    let live_worktree =
        utils::git::worktrees::session::SessionWorktree::create(&project, &live.id, None)
            .unwrap()
            .unwrap();
    live.record(crate::session::Event::Worktree(Some(live_worktree.clone())))
        .unwrap();
    std::fs::write(live_worktree.path.join("lib.rs"), "live edits\n").unwrap();
    let locked = h
        .store
        .create(llm::SessionProvider::Injected, None, Vec::new())
        .unwrap();
    let locked_worktree =
        utils::git::worktrees::session::SessionWorktree::create(&project, &locked.id, None)
            .unwrap()
            .unwrap();
    locked
        .record(crate::session::Event::Worktree(Some(
            locked_worktree.clone(),
        )))
        .unwrap();
    std::fs::write(locked_worktree.path.join("local.txt"), "local\n").unwrap();
    let child = git2::Repository::open(&locked_worktree.path).unwrap();
    let lock = child.path().join("locked");
    std::fs::write(&lock, "keep\n").unwrap();
    let locked_id = locked.id.clone();
    drop(locked);
    let inactive = h
        .store
        .list()
        .unwrap()
        .into_iter()
        .find(|snapshot| snapshot.id != live.id && snapshot.id != locked_id)
        .unwrap();
    let inactive_worktree = inactive.worktree.unwrap();
    std::fs::write(inactive_worktree.path.join("local.txt"), "discard\n").unwrap();
    h.command(Command::New).await;
    let message = h.command(Command::Prune(PruneMode::Force)).await;
    assert!(message.contains("Pruned 1"), "{message}");
    assert!(
        message.contains(&format!("Skipped {}", live.id)),
        "{message}"
    );
    assert!(
        message.contains(&format!("Skipped {locked_id}")),
        "{message}"
    );
    assert!(!inactive_worktree.path.exists());
    assert!(live.snapshot().unwrap().worktree.is_some());
    assert!(live_worktree.path.exists());
    assert!(h.snapshot(&locked_id).worktree.is_some());
    assert!(locked_worktree.path.exists());
    std::fs::remove_file(lock).unwrap();
    assert!(
        h.command(Command::Prune(PruneMode::Force))
            .await
            .contains("Pruned 1")
    );
    assert!(h.snapshot(&locked_id).worktree.is_none());
    drop(live);
    assert!(
        h.command(Command::Prune(PruneMode::Force))
            .await
            .contains("Pruned 1")
    );
    assert!(!live_worktree.path.exists());
    h.stop().await;
}

#[tokio::test]
async fn prune_recovers_interrupted_cleanup_and_clears_saved_worktree_state() {
    let h = GitHarness::new().await;
    let id = h.store.list().unwrap()[0].id.clone();
    let worktree = h.snapshot(&id).worktree.unwrap();
    h.complete(Some(PATCH)).await;
    h.command(Command::New).await;
    let reference = format!("refs/heads/joe/session/{id}");
    let mut options = git2::WorktreePruneOptions::new();
    options.valid(true).working_tree(true);
    h.repo
        .find_worktree(&id)
        .unwrap()
        .prune(Some(&mut options))
        .unwrap();
    assert!(h.repo.find_reference(&reference).is_ok());
    let mut lock = h.repo.transaction().unwrap();
    lock.lock_ref(&reference).unwrap();
    let message = h.command(Command::Prune(PruneMode::Force)).await;
    assert!(message.contains("Pruned 0"), "{message}");
    assert!(h.snapshot(&id).worktree.is_some());
    drop(lock);
    let message = h.command(Command::Prune(PruneMode::Merged)).await;
    assert!(message.contains("Pruned 0"), "{message}");
    assert!(message.contains("use /prune --force"), "{message}");
    assert!(h.snapshot(&id).worktree.is_some());
    assert!(h.repo.find_reference(&reference).is_ok());
    let message = h.command(Command::Prune(PruneMode::Force)).await;
    assert!(message.contains("Pruned 1"), "{message}");
    let snapshot = h.snapshot(&id);
    assert!(snapshot.worktree.is_none());
    assert!(matches!(
        snapshot.merge_approval,
        crate::session::session_merge::MergeApproval::None
    ));
    assert!(h.repo.find_reference(&reference).is_err());
    assert!(
        h.command(Command::Prune(PruneMode::Force))
            .await
            .contains("Pruned 0")
    );
    h.actor
        .send_message(Message::Command(Command::Resume(ResumeTarget::Session {
            id,
        })))
        .unwrap();
    assert!(matches!(
        h.event(|packet| matches!(packet, ActorToTuiPacket::SessionResumed(_)))
            .await,
        ActorToTuiPacket::SessionResumed(Ok(_))
    ));
    assert!(worktree.path.exists());
    h.stop().await;
}

#[tokio::test]
async fn prune_rejects_plan_mode_and_active_turns() {
    let h = GitHarness::new().await;
    let id = h.store.list().unwrap()[0].id.clone();
    let worktree = h.snapshot(&id).worktree.unwrap();
    std::fs::write(worktree.path.join("local.txt"), "unmerged\n").unwrap();
    h.command(Command::New).await;
    h.command(Command::Plan).await;
    for mode in [PruneMode::Merged, PruneMode::Force] {
        let message = h.command(Command::Prune(mode)).await;
        assert!(message.contains("Plan mode"), "{message}");
    }
    assert!(worktree.path.exists());
    h.command(Command::Implement).await;
    h.actor
        .send_message(Message::StartWork(Some("Inspect".into())))
        .unwrap();
    let (_, reply) = within(h.requests.recv_async()).await.unwrap();
    for mode in [PruneMode::Merged, PruneMode::Force] {
        let message = h.command(Command::Prune(mode)).await;
        assert!(message.contains("Interrupt the active turn"), "{message}");
    }
    assert!(worktree.path.exists());
    h.actor.send_message(Message::Interrupt).unwrap();
    h.event(|packet| {
        matches!(
            packet,
            ActorToTuiPacket::TurnChanged {
                state: Lifecycle::Cancelled,
                ..
            }
        )
    })
    .await;
    drop(reply);
    assert!(
        h.command(Command::Prune(PruneMode::Force))
            .await
            .contains("Pruned 1")
    );
    h.stop().await;
}

#[tokio::test]
async fn invalid_commit_subjects_do_not_block_approved_merges() {
    let h = GitHarness::new().await;
    let base = h.repo.refname_to_id("HEAD").unwrap();
    let question = h.complete_with_subject(Some(PATCH), "").await;
    assert_eq!(h.repo.refname_to_id("HEAD").unwrap(), base);
    assert!(
        h.answer_merge(&question, "merge")
            .await
            .contains("Cleaned up")
    );
    assert_eq!(
        h.repo
            .head()
            .unwrap()
            .peel_to_commit()
            .unwrap()
            .message()
            .unwrap(),
        "Update lib.rs"
    );
    h.stop().await;
}

#[tokio::test]
async fn unchanged_tasks_do_not_request_a_commit_subject_or_merge() {
    let h = GitHarness::new().await;
    let id = h.store.list().unwrap()[0].id.clone();
    let base = h.repo.refname_to_id("HEAD").unwrap();
    h.actor
        .send_message(Message::StartWork(Some("Inspect the function".into())))
        .unwrap();
    let (_, reply) = within(h.requests.recv_async()).await.unwrap();
    answer(reply, response(vec![text("No changes needed")]));
    h.event(|packet| {
        matches!(
            packet,
            ActorToTuiPacket::TurnChanged {
                state: Lifecycle::Completed,
                ..
            }
        )
    })
    .await;
    let (reply, receive) = oneshot::channel();
    h.actor
        .send_message(Message::Inspect(reply.into()))
        .unwrap();
    within(receive).await.unwrap();
    assert!(h.requests.is_empty());
    assert_eq!(h.repo.refname_to_id("HEAD").unwrap(), base);
    assert!(matches!(
        h.snapshot(&id).merge_approval,
        crate::session::session_merge::MergeApproval::None
    ));
    h.stop().await;
}

#[tokio::test]
async fn approving_a_conflicted_merge_resolves_and_merges_without_another_question() {
    let h = GitHarness::new().await;
    let id = h.store.list().unwrap()[0].id.clone();
    let worktree = h.snapshot(&id).worktree.unwrap();
    let question = h.complete(Some(PATCH)).await;
    let target = h.commit_main("pub fn value() -> u32 { 3 }\n");
    let message = h.answer_merge(&question, "merge").await;
    assert!(message.contains("Resolving merge conflicts"), "{message}");
    let (request, reply) = within(h.requests.recv_async()).await.unwrap();
    assert!(request.messages.iter().any(|message| {
        message
            .text()
            .contains("Do not ask for merge approval again")
    }));
    assert!(request.messages.iter().any(|message| {
        message
            .text()
            .contains(&format!("Answer to question {}", question.id))
    }));
    assert!(h.snapshot(&id).questions.pending().is_empty());
    assert_eq!(h.repo.refname_to_id("HEAD").unwrap(), target);
    let conflicted = std::fs::read_to_string(worktree.path.join("lib.rs")).unwrap();
    assert!(conflicted.contains("<<<<<<<"));
    answer(
        reply,
        response(vec![read_call(
            std::path::Path::new("lib.rs"),
            "read-conflicts",
        )]),
    );
    let (_, reply) = within(h.requests.recv_async()).await.unwrap();
    let patch = format!(
        "*** Begin Patch\n*** Update File: lib.rs\n@@\n{}+pub fn value() -> u32 {{ 5 }}\n*** End Patch",
        conflicted
            .lines()
            .map(|line| format!("-{line}\n"))
            .collect::<String>()
    );
    let mut block = call("apply_patch", "resolve");
    if let ContentBlock::ToolBlock { input, .. } = &mut block {
        *input = json!({ "patch": patch }).as_object().unwrap().clone();
    }
    answer(reply, response(vec![block]));
    let (request, reply) = within(h.requests.recv_async()).await.unwrap();
    assert!(
        request
            .messages
            .iter()
            .all(|message| message.content.iter().all(|content| !matches!(
                content,
                ContentBlock::ToolResult {
                    is_error: Some(true),
                    ..
                }
            ))),
        "{:?}",
        request.messages
    );
    answer(reply, response(vec![text("Conflicts resolved")]));
    let summary = h
        .summarize("Resolve conflicting return values by returning 5")
        .await;
    assert!(
        summary.messages[0]
            .text()
            .contains("-pub fn value() -> u32 { 3 }")
    );
    assert!(
        summary.messages[0]
            .text()
            .contains("+pub fn value() -> u32 { 5 }")
    );
    let packet = h
        .event(|packet| match packet {
            ActorToTuiPacket::ContextNotice(message) => {
                message.contains("Merged session into main")
            }
            ActorToTuiPacket::SessionError(_) => true,
            _ => false,
        })
        .await;
    assert!(
        matches!(packet, ActorToTuiPacket::ContextNotice(_)),
        "{packet:?}"
    );
    assert!(
        std::fs::read_to_string(h.workspace.path.join("lib.rs"))
            .unwrap()
            .contains("{ 5 }")
    );
    assert!(matches!(
        h.snapshot(&id).merge_approval,
        crate::session::session_merge::MergeApproval::None
    ));
    assert!(h.snapshot(&id).worktree.is_none());
    assert!(!worktree.path.exists());
    assert!(h.repo.find_worktree(&id).is_err());
    assert!(
        h.repo
            .find_branch(&format!("joe/session/{id}"), git2::BranchType::Local)
            .is_err()
    );
    assert_eq!(
        h.repo
            .head()
            .unwrap()
            .peel_to_commit()
            .unwrap()
            .parent_count(),
        2
    );
    assert_eq!(
        h.repo
            .head()
            .unwrap()
            .peel_to_commit()
            .unwrap()
            .message()
            .unwrap(),
        "Resolve conflicting return values by returning 5"
    );
    h.stop().await;
}

#[tokio::test]
async fn incomplete_conflict_resolution_cannot_merge_even_after_model_completion() {
    let h = GitHarness::new().await;
    let id = h.store.list().unwrap()[0].id.clone();
    let worktree = h.snapshot(&id).worktree.unwrap();
    let question = h.complete(Some(PATCH)).await;
    let target = h.commit_main("pub fn value() -> u32 { 3 }\n");
    assert!(
        h.answer_merge(&question, "merge")
            .await
            .contains("Resolving merge conflicts")
    );
    let (_, reply) = within(h.requests.recv_async()).await.unwrap();
    answer(reply, response(vec![text("Conflicts resolved")]));
    let packet = h
        .event(|packet| matches!(packet, ActorToTuiPacket::SessionError(_)))
        .await;
    assert!(
        matches!(packet, ActorToTuiPacket::SessionError(message) if message.contains("Unresolved conflict markers"))
    );
    assert_eq!(h.repo.refname_to_id("HEAD").unwrap(), target);
    assert!(h.snapshot(&id).worktree.is_some());
    assert!(worktree.path.exists());
    assert!(
        h.repo
            .find_branch(&format!("joe/session/{id}"), git2::BranchType::Local)
            .is_ok()
    );
    h.stop().await;
}

#[tokio::test]
async fn interrupting_conflict_resolution_keeps_main_unchanged() {
    let h = GitHarness::new().await;
    let id = h.store.list().unwrap()[0].id.clone();
    let question = h.complete(Some(PATCH)).await;
    let target = h.commit_main("pub fn value() -> u32 { 3 }\n");
    assert!(
        h.answer_merge(&question, "merge")
            .await
            .contains("Resolving merge conflicts")
    );
    let (_, reply) = within(h.requests.recv_async()).await.unwrap();
    h.actor.send_message(Message::Interrupt).unwrap();
    h.event(|packet| {
        matches!(
            packet,
            ActorToTuiPacket::TurnChanged {
                state: Lifecycle::Cancelled,
                ..
            }
        )
    })
    .await;
    drop(reply);
    assert_eq!(h.repo.refname_to_id("HEAD").unwrap(), target);
    assert!(matches!(
        h.snapshot(&id).merge_approval,
        crate::session::session_merge::MergeApproval::Resolving {
            activity: crate::session::session_merge::ResolutionActivity::Paused,
            ..
        }
    ));
    h.stop().await;
}

#[tokio::test]
async fn merge_questions_survive_session_switches_and_failed_tasks_revoke_approval() {
    let h = GitHarness::new().await;
    let id = h.store.list().unwrap()[0].id.clone();
    let base = h.repo.refname_to_id("HEAD").unwrap();
    let question = h.complete(Some(PATCH)).await;
    h.command(Command::New).await;
    h.actor
        .send_message(Message::Command(Command::Resume(ResumeTarget::Session {
            id: id.clone(),
        })))
        .unwrap();
    assert!(matches!(
        h.event(|packet| matches!(packet, ActorToTuiPacket::SessionResumed(_)))
            .await,
        ActorToTuiPacket::SessionResumed(Ok(_))
    ));
    assert_eq!(
        h.snapshot(&id).merge_approval.question().unwrap().id,
        question.id
    );
    assert_eq!(h.snapshot(&id).questions.pending(), &[question.clone()]);
    h.command(Command::Plan).await;
    assert!(
        h.answer_merge(&question, "merge")
            .await
            .contains("Plan mode")
    );
    assert_eq!(h.snapshot(&id).questions.pending(), &[question.clone()]);
    assert!(
        !h.snapshot(&id)
            .planning
            .evidence
            .contains_key(&format!("answer:{}", question.id))
    );
    h.command(Command::Implement).await;
    h.actor
        .send_message(Message::StartWork(Some("Continue the task".into())))
        .unwrap();
    let (_, reply) = within(h.requests.recv_async()).await.unwrap();
    assert!(
        reply
            .send(Err(Failure::new(
                FailureKind::Authentication,
                "Fixture provider failed"
            )
            .into()))
            .is_ok()
    );
    h.event(|packet| {
        matches!(
            packet,
            ActorToTuiPacket::TurnChanged {
                state: Lifecycle::Failed,
                ..
            }
        )
    })
    .await;
    assert!(matches!(
        h.snapshot(&id).merge_approval,
        crate::session::session_merge::MergeApproval::None
    ));
    assert!(h.snapshot(&id).questions.pending().is_empty());
    assert_eq!(h.repo.refname_to_id("HEAD").unwrap(), base);
    assert!(
        h.answer_merge(&question, "merge")
            .await
            .contains("not pending")
    );
    assert_eq!(h.repo.refname_to_id("HEAD").unwrap(), base);
    h.stop().await;
}

#[tokio::test]
async fn successful_tasks_prompt_and_only_explicit_acceptance_updates_main() {
    let h = GitHarness::new().await;
    let id = h.store.list().unwrap()[0].id.clone();
    let worktree = h.snapshot(&id).worktree.unwrap();
    let base = h.repo.refname_to_id("HEAD").unwrap();
    let question = h.complete(Some(PATCH)).await;
    assert!(question.prompt.contains("main"));
    assert_eq!(question.purpose, QuestionPurpose::Merge);
    assert_eq!(h.snapshot(&id).questions.pending(), &[question.clone()]);
    assert!(h.command(Command::Questions).await.contains(&question.id));
    assert_eq!(h.repo.refname_to_id("HEAD").unwrap(), base);
    assert!(
        std::fs::read_to_string(worktree.path.join("lib.rs"))
            .unwrap()
            .contains("{ 2 }")
    );
    assert!(
        std::fs::read_to_string(h.workspace.path.join("lib.rs"))
            .unwrap()
            .contains("{ 1 }")
    );
    assert!(
        h.answer_merge(&question, "keep")
            .await
            .contains("Kept changes")
    );
    let kept = h.snapshot(&id);
    assert!(kept.questions.pending().is_empty());
    assert_eq!(
        kept.planning.evidence[&format!("answer:{}", question.id)],
        "Keep changes in this session"
    );
    assert!(kept.history.iter().any(|message| {
        message
            .text()
            .contains(&format!("Answer to question {}", question.id))
    }));
    assert!(
        h.answer_merge(&question, "merge")
            .await
            .contains("not pending")
    );
    assert_eq!(h.repo.refname_to_id("HEAD").unwrap(), base);
    let question = h.complete(None).await;
    assert!(h.snapshot(&id).worktree.is_some());
    assert!(worktree.path.exists());
    let message = h.answer_merge(&question, "merge").await;
    assert!(message.contains("Merged session into main"), "{message}");
    assert!(
        message.contains("Cleaned up the session workspace and branch"),
        "{message}"
    );
    assert!(h.snapshot(&id).worktree.is_none());
    let merged = h.snapshot(&id);
    assert!(merged.questions.pending().is_empty());
    assert_eq!(
        merged.planning.evidence[&format!("answer:{}", question.id)],
        "Merge into main"
    );
    assert!(merged.history.iter().any(|message| {
        message
            .text()
            .contains(&format!("Answer to question {}", question.id))
    }));
    assert!(!worktree.path.exists());
    assert!(h.repo.find_worktree(&id).is_err());
    assert!(
        h.repo
            .find_branch(&format!("joe/session/{id}"), git2::BranchType::Local)
            .is_err()
    );
    assert_ne!(h.repo.refname_to_id("HEAD").unwrap(), base);
    assert_eq!(
        h.repo
            .head()
            .unwrap()
            .peel_to_commit()
            .unwrap()
            .message()
            .unwrap(),
        "Make value return 2 instead of 1"
    );
    assert!(
        std::fs::read_to_string(h.workspace.path.join("lib.rs"))
            .unwrap()
            .contains("{ 2 }")
    );
    assert!(matches!(
        h.snapshot(&id).merge_approval,
        crate::session::session_merge::MergeApproval::None
    ));
    h.stop().await;
}

#[tokio::test]
async fn failed_merges_record_the_answer_and_offer_a_fresh_retry() {
    let h = GitHarness::new().await;
    let id = h.store.list().unwrap()[0].id.clone();
    let base = h.repo.refname_to_id("HEAD").unwrap();
    let question = h.complete(Some(PATCH)).await;
    std::fs::write(h.workspace.path.join("lib.rs"), "uncommitted main edit\n").unwrap();
    let message = h.answer_merge(&question, "merge").await;
    assert!(!message.contains("Cleaned up"), "{message}");
    assert_eq!(h.repo.refname_to_id("HEAD").unwrap(), base);
    let snapshot = h.snapshot(&id);
    assert_eq!(
        snapshot.planning.evidence[&format!("answer:{}", question.id)],
        "Merge into main"
    );
    let retry = snapshot.questions.pending()[0].clone();
    assert_ne!(retry.id, question.id);
    assert_eq!(retry.prompt, question.prompt);
    assert!(
        h.answer_merge(&question, "merge")
            .await
            .contains("not pending")
    );
    let message = h
        .command(Command::Answer(QuestionAnswer {
            id: retry.id.clone(),
            answer: Answer::Text("merge".into()),
        }))
        .await;
    assert!(message.contains("requires a listed choice"), "{message}");
    assert_eq!(h.snapshot(&id).questions.pending(), &[retry.clone()]);
    std::fs::write(
        h.workspace.path.join("lib.rs"),
        "pub fn value() -> u32 { 1 }\n",
    )
    .unwrap();
    assert!(h.answer_merge(&retry, "merge").await.contains("Cleaned up"));
    h.stop().await;
}

#[tokio::test]
async fn forks_withdraw_merge_questions_without_affecting_the_parent() {
    let h = GitHarness::new().await;
    let original = h.store.list().unwrap()[0].id.clone();
    let question = h.complete(Some(PATCH)).await;
    assert!(
        h.command(Command::Fork)
            .await
            .contains("Forked conversation")
    );
    let fork = h
        .store
        .list()
        .unwrap()
        .into_iter()
        .find(|snapshot| snapshot.id != original)
        .unwrap();
    assert!(fork.questions.pending().is_empty());
    assert!(matches!(
        fork.merge_approval,
        crate::session::session_merge::MergeApproval::None
    ));
    assert_eq!(
        h.snapshot(&original).questions.pending(),
        &[question.clone()]
    );
    assert!(
        h.answer_merge(&question, "merge")
            .await
            .contains("not pending")
    );
    h.stop().await;
}

#[tokio::test]
async fn legacy_and_interrupted_merge_approvals_restore_as_shared_questions() {
    use crate::session::{Event, ResumableSession, session_merge::MergeApproval};
    enum SavedApproval {
        Legacy,
        Interrupted,
    }
    for saved in [SavedApproval::Legacy, SavedApproval::Interrupted] {
        let h = GitHarness::new().await;
        let id = h.store.list().unwrap()[0].id.clone();
        let base = h.repo.refname_to_id("HEAD").unwrap();
        let question = h.complete(Some(PATCH)).await;
        let commit = match h.snapshot(&id).merge_approval {
            MergeApproval::Awaiting { commit, .. } => commit,
            _ => panic!("Expected pending merge"),
        };
        h.command(Command::New).await;
        let session = ResumableSession::new(
            &h.store,
            &id,
            &utils::workspace::WorkspacePolicy::workspace(h.workspace.path.clone()).unwrap(),
            &llm::SessionProvider::Injected,
        )
        .unwrap()
        .resume()
        .unwrap();
        session
            .record(Event::QuestionsWithdrawn(QuestionPurpose::Merge))
            .unwrap();
        let approval = match saved {
            SavedApproval::Legacy => MergeApproval::Awaiting {
                question: "legacy-merge".into(),
                commit,
            },
            SavedApproval::Interrupted => MergeApproval::Approved { commit },
        };
        session.record(Event::MergeApproval(approval)).unwrap();
        drop(session);
        h.actor
            .send_message(Message::Command(Command::Resume(ResumeTarget::Session {
                id: id.clone(),
            })))
            .unwrap();
        assert!(matches!(
            h.event(|packet| matches!(packet, ActorToTuiPacket::SessionResumed(_)))
                .await,
            ActorToTuiPacket::SessionResumed(Ok(_))
        ));
        let restored = h.snapshot(&id).questions.pending()[0].clone();
        assert_eq!(restored.purpose, QuestionPurpose::Merge);
        assert_eq!(restored.prompt, question.prompt);
        assert_eq!(h.repo.refname_to_id("HEAD").unwrap(), base);
        assert!(h.command(Command::Questions).await.contains(&restored.id));
        assert!(
            h.answer_merge(&restored, "merge")
                .await
                .contains("Cleaned up")
        );
        h.stop().await;
    }
}

#[tokio::test]
async fn new_fork_and_resume_keep_distinct_workspaces_and_switch_context() {
    let h = GitHarness::new().await;
    let original = h.store.list().unwrap()[0].id.clone();
    let worktree = h.snapshot(&original).worktree.unwrap();
    let question = h.complete(Some(PATCH)).await;
    h.answer_merge(&question, "keep").await;
    let message = h.command(Command::Fork).await;
    assert!(message.contains("Forked conversation"), "{message}");
    let fork = h
        .store
        .list()
        .unwrap()
        .into_iter()
        .find(|snapshot| snapshot.id != original)
        .unwrap();
    let fork_tree = fork.worktree.unwrap();
    assert_ne!(fork_tree.path, worktree.path);
    assert_eq!(
        std::fs::read_to_string(fork_tree.path.join("lib.rs")).unwrap(),
        std::fs::read_to_string(worktree.path.join("lib.rs")).unwrap()
    );
    assert!(
        h.command(Command::New)
            .await
            .contains("Started a new session")
    );
    let fresh = h
        .store
        .list()
        .unwrap()
        .into_iter()
        .find(|snapshot| snapshot.id != original && snapshot.id != fork.id)
        .unwrap()
        .worktree
        .unwrap();
    assert!(
        std::fs::read_to_string(fresh.path.join("lib.rs"))
            .unwrap()
            .contains("{ 1 }")
    );
    h.actor
        .send_message(Message::Command(Command::Resume(ResumeTarget::Session {
            id: original.clone(),
        })))
        .unwrap();
    assert!(matches!(
        h.event(|packet| matches!(packet, ActorToTuiPacket::SessionResumed(_)))
            .await,
        ActorToTuiPacket::SessionResumed(Ok(_))
    ));
    h.actor
        .send_message(Message::StartWork(Some("Inspect resumed workspace".into())))
        .unwrap();
    let (request, reply) = within(h.requests.recv_async()).await.unwrap();
    assert!(
        request.messages[0]
            .text()
            .contains(worktree.path.to_str().unwrap())
    );
    h.actor.send_message(Message::Interrupt).unwrap();
    h.event(|packet| {
        matches!(
            packet,
            ActorToTuiPacket::TurnChanged {
                state: Lifecycle::Cancelled,
                ..
            }
        )
    })
    .await;
    drop(reply);
    assert!(matches!(
        h.snapshot(&original).merge_approval,
        crate::session::session_merge::MergeApproval::None
    ));
    assert!(
        std::fs::read_to_string(h.workspace.path.join("lib.rs"))
            .unwrap()
            .contains("{ 1 }")
    );
    h.stop().await;
}

#[tokio::test]
async fn another_task_after_merge_gets_a_fresh_isolated_workspace() {
    let h = GitHarness::new().await;
    let id = h.store.list().unwrap()[0].id.clone();
    let worktree = h.snapshot(&id).worktree.unwrap();
    let cache = worktree
        .path
        .join("target/.joe/linux/build/debug/build.bin");
    std::fs::create_dir_all(cache.parent().unwrap()).unwrap();
    std::fs::File::create(&cache)
        .unwrap()
        .set_len(128 * 1024 * 1024)
        .unwrap();
    let question = h.complete(Some(PATCH)).await;
    assert!(
        h.answer_merge(&question, "merge")
            .await
            .contains("Cleaned up")
    );
    assert!(h.snapshot(&id).worktree.is_none());
    assert!(!worktree.path.exists());
    assert!(h.repo.find_worktree(&id).is_err());
    assert!(
        h.repo
            .find_branch(&format!("joe/session/{id}"), git2::BranchType::Local)
            .is_err()
    );
    assert!(matches!(
        h.snapshot(&id).merge_approval,
        crate::session::session_merge::MergeApproval::None
    ));
    let merged = h.repo.refname_to_id("HEAD").unwrap();
    let history = h.snapshot(&id).history.len();
    let patch = "*** Begin Patch\n*** Update File: lib.rs\n@@\n-pub fn value() -> u32 { 2 }\n+pub fn value() -> u32 { 4 }\n*** End Patch";
    let question = h.complete(Some(patch)).await;
    assert_eq!(h.snapshot(&id).worktree.unwrap().path, worktree.path);
    assert!(!cache.exists());
    assert!(h.snapshot(&id).history.len() > history);
    assert_eq!(h.repo.refname_to_id("HEAD").unwrap(), merged);
    assert!(
        std::fs::read_to_string(h.workspace.path.join("lib.rs"))
            .unwrap()
            .contains("{ 2 }")
    );
    assert!(
        std::fs::read_to_string(worktree.path.join("lib.rs"))
            .unwrap()
            .contains("{ 4 }")
    );
    assert!(
        h.answer_merge(&question, "merge")
            .await
            .contains("Cleaned up")
    );
    assert!(!worktree.path.exists());
    assert!(h.snapshot(&id).worktree.is_none());
    assert!(
        std::fs::read_to_string(h.workspace.path.join("lib.rs"))
            .unwrap()
            .contains("{ 4 }")
    );
    h.stop().await;
}

#[tokio::test]
async fn merged_session_can_be_resumed_from_current_main() {
    let h = GitHarness::new().await;
    let id = h.store.list().unwrap()[0].id.clone();
    let worktree = h.snapshot(&id).worktree.unwrap();
    let question = h.complete(Some(PATCH)).await;
    assert!(
        h.answer_merge(&question, "merge")
            .await
            .contains("Cleaned up")
    );
    h.command(Command::New).await;
    let main = h.commit_main("pub fn value() -> u32 { 8 }\n");
    h.actor
        .send_message(Message::Command(Command::Resume(ResumeTarget::Session {
            id: id.clone(),
        })))
        .unwrap();
    assert!(matches!(
        h.event(|packet| matches!(packet, ActorToTuiPacket::SessionResumed(_)))
            .await,
        ActorToTuiPacket::SessionResumed(Ok(_))
    ));
    assert_eq!(h.snapshot(&id).worktree.unwrap().path, worktree.path);
    assert_eq!(
        git2::Repository::open(&worktree.path)
            .unwrap()
            .refname_to_id("HEAD")
            .unwrap(),
        main
    );
    h.actor
        .send_message(Message::StartWork(Some(
            "Inspect the resumed workspace".into(),
        )))
        .unwrap();
    let (request, reply) = within(h.requests.recv_async()).await.unwrap();
    assert!(
        request.messages[0]
            .text()
            .contains(worktree.path.to_str().unwrap())
    );
    answer(reply, response(vec![text("Inspected")]));
    h.event(|packet| {
        matches!(
            packet,
            ActorToTuiPacket::TurnChanged {
                state: Lifecycle::Completed,
                ..
            }
        )
    })
    .await;
    h.stop().await;
}

#[tokio::test]
async fn cleanup_failure_reports_successful_merge_and_preserves_data_for_retry() {
    let h = GitHarness::new().await;
    let id = h.store.list().unwrap()[0].id.clone();
    let worktree = h.snapshot(&id).worktree.unwrap();
    std::fs::write(worktree.path.join(".gitignore"), "private.txt\n").unwrap();
    let question = h.complete(Some(PATCH)).await;
    std::fs::write(worktree.path.join("private.txt"), "private data\n").unwrap();
    let child = git2::Repository::open(&worktree.path).unwrap();
    let lock = child.path().join("locked");
    std::fs::write(&lock, "keep this worktree\n").unwrap();
    let message = h.answer_merge(&question, "merge").await;
    assert!(message.contains("Merged session into main"), "{message}");
    assert!(message.contains("Cleanup could not finish"), "{message}");
    assert!(!message.contains("Cleaned up"), "{message}");
    assert!(h.snapshot(&id).worktree.is_some());
    assert_eq!(
        std::fs::read_to_string(worktree.path.join("private.txt")).unwrap(),
        "private data\n"
    );
    assert!(
        h.repo
            .find_branch(&format!("joe/session/{id}"), git2::BranchType::Local)
            .is_ok()
    );
    assert!(
        std::fs::read_to_string(h.workspace.path.join("lib.rs"))
            .unwrap()
            .contains("{ 2 }")
    );
    std::fs::remove_file(lock).unwrap();
    let retry = h.snapshot(&id).questions.pending()[0].clone();
    assert_ne!(retry.id, question.id);
    assert!(
        h.answer_merge(&question, "merge")
            .await
            .contains("not pending")
    );
    let message = h.answer_merge(&retry, "merge").await;
    assert!(message.contains("already in main"), "{message}");
    assert!(message.contains("Cleaned up"), "{message}");
    assert!(h.snapshot(&id).worktree.is_none());
    assert!(!worktree.path.exists());
    assert!(h.repo.find_worktree(&id).is_err());
    assert!(
        h.repo
            .find_branch(&format!("joe/session/{id}"), git2::BranchType::Local)
            .is_err()
    );
    h.stop().await;
}
