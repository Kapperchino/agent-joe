use super::*;
use analysis::contexts::rust_context::RustContext;
use commands::command::{Answer, Command, QuestionAnswer, ResumeTarget};

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
async fn approving_a_conflicted_merge_resolves_and_merges_without_another_question() {
    let h = GitHarness::new().await;
    let id = h.store.list().unwrap()[0].id.clone();
    let worktree = h.snapshot(&id).worktree.unwrap();
    let question = h.complete(Some(PATCH)).await;
    let target = h.commit_main("pub fn value() -> u32 { 3 }\n");
    let message = h.answer_merge(&question, "merge").await;
    assert!(message.contains("Resolving merge conflicts"), "{message}");
    let (request, reply) = within(h.requests.recv_async()).await.unwrap();
    assert!(
        request
            .messages
            .last()
            .unwrap()
            .text()
            .contains("Do not ask for merge approval again")
    );
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
        crate::session_merge::MergeApproval::None
    ));
    assert_eq!(
        h.repo
            .head()
            .unwrap()
            .peel_to_commit()
            .unwrap()
            .parent_count(),
        2
    );
    h.stop().await;
}

#[tokio::test]
async fn incomplete_conflict_resolution_cannot_merge_even_after_model_completion() {
    let h = GitHarness::new().await;
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
        crate::session_merge::MergeApproval::Resolving {
            activity: crate::session_merge::ResolutionActivity::Paused,
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
    h.command(Command::Plan).await;
    assert!(
        h.answer_merge(&question, "merge")
            .await
            .contains("Plan mode")
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
        crate::session_merge::MergeApproval::None
    ));
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
    assert_eq!(h.repo.refname_to_id("HEAD").unwrap(), base);
    let question = h.complete(None).await;
    assert!(
        h.answer_merge(&question, "merge")
            .await
            .contains("Merged session into main")
    );
    assert_ne!(h.repo.refname_to_id("HEAD").unwrap(), base);
    assert!(
        std::fs::read_to_string(h.workspace.path.join("lib.rs"))
            .unwrap()
            .contains("{ 2 }")
    );
    assert!(matches!(
        h.snapshot(&id).merge_approval,
        crate::session_merge::MergeApproval::None
    ));
    h.stop().await;
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
        crate::session_merge::MergeApproval::None
    ));
    assert!(
        std::fs::read_to_string(h.workspace.path.join("lib.rs"))
            .unwrap()
            .contains("{ 1 }")
    );
    h.stop().await;
}
