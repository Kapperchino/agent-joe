use super::*;
use analysis::contexts::rust_context::RustContext;
use commands::command::{Answer, Command, PruneMode, QuestionAnswer, ResumeTarget};
use common_models::interaction::QuestionPurpose;

struct GitHarness {
    workspace: session::test_support::Workspace,
    repo: git2::Repository,
    actor: ActorRef<Message>,
    handle: tokio::task::JoinHandle<()>,
    store: Arc<session::SessionStore>,
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
        let workspace = session::test_support::Workspace::new();
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
                        crate::tools::knowledge::Knowledge,
                        RustContext,
                        ActorContext<RustContext>,
                    >(),
                    tools::tool_defs::erased_tool::<
                        tools::review_changes::ReviewChanges,
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

    fn snapshot(&self, id: &str) -> session::Snapshot {
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

    async fn interrupt(&self) {
        self.actor.send_message(Message::Interrupt).unwrap();
        self.event(|packet| {
            matches!(
                packet,
                ActorToTuiPacket::TurnChanged {
                    state: Lifecycle::Cancelled,
                    ..
                }
            )
        })
        .await;
    }

    async fn write_and_interrupt(&self, patch: &str) {
        self.actor
            .send_message(Message::StartWork(Some("Apply the fixture edit".into())))
            .unwrap();
        let (_, reply) = within(self.requests.recv_async()).await.unwrap();
        answer(
            reply,
            response(vec![tool_call(
                "apply_patch",
                "edit",
                json!({"patch": patch}),
            )]),
        );
        let (request, reply) = within(self.requests.recv_async()).await.unwrap();
        assert_tool_success(latest_tool_result(&request));
        self.interrupt().await;
        drop(reply);
    }

    async fn read_and_interrupt(&self, root: &std::path::Path, expected: &str) {
        self.actor
            .send_message(Message::StartWork(Some("Inspect the function".into())))
            .unwrap();
        let (request, reply) = within(self.requests.recv_async()).await.unwrap();
        assert!(request.messages[0].text().contains(root.to_str().unwrap()));
        answer(
            reply,
            response(vec![tool_call(
                "knowledge",
                "read-source",
                json!({"action":"read", "file_path":"lib.rs"}),
            )]),
        );
        let (request, reply) = within(self.requests.recv_async()).await.unwrap();
        let result = latest_tool_result(&request);
        assert_tool_success(result);
        assert!(format!("{result:?}").contains(expected), "{result:?}");
        self.interrupt().await;
        drop(reply);
    }

    async fn complete(&self, patch: Option<&str>) -> common_models::interaction::Question {
        self.complete_with_subject(patch, "Make value return 2 instead of 1")
            .await
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
                assert_tool_success(latest_tool_result(&request));
                reply
            }
            None => reply,
        };
        answer(reply, response(vec![review_call(subject, "review-final")]));
        let (request, reply) = within(self.requests.recv_async()).await.unwrap();
        assert_tool_success(latest_tool_result(&request));
        answer(reply, response(vec![text("Task completed")]));
        self.merge_question().await
    }

    async fn merge_question(&self) -> common_models::interaction::Question {
        let packet = self
            .event(|packet| match packet {
                ActorToTuiPacket::InteractionUpdated(view) => view
                    .questions
                    .iter()
                    .any(|question| question.id.starts_with("merge-")),
                ActorToTuiPacket::SessionError(_) => true,
                _ => false,
            })
            .await;
        assert!(self.requests.is_empty());
        match packet {
            ActorToTuiPacket::InteractionUpdated(view) => view
                .questions
                .into_iter()
                .find(|question| question.id.starts_with("merge-"))
                .unwrap(),
            packet => panic!("Unexpected merge response: {packet:?}"),
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
const NOOP_PATCH: &str = "*** Begin Patch\n*** Update File: lib.rs\n@@\n-pub fn value() -> u32 { 1 }\n+pub fn value() -> u32 { 1 }\n*** End Patch";

fn tool_call(name: &str, id: &str, arguments: Value) -> ContentBlock {
    let mut block = call(name, id);
    if let ContentBlock::ToolBlock { input, .. } = &mut block {
        *input = arguments.as_object().unwrap().clone();
    }
    block
}

fn assert_tool_success(result: &ContentBlock) {
    assert!(
        matches!(
            result,
            ContentBlock::ToolResult {
                is_error: None | Some(false),
                ..
            }
        ),
        "{result:?}"
    );
}

fn review_call(subject: &str, id: &str) -> ContentBlock {
    let mut block = call("review_changes", id);
    if let ContentBlock::ToolBlock { input, .. } = &mut block {
        *input = json!({ "commit_message": subject })
            .as_object()
            .unwrap()
            .clone();
    }
    block
}

#[tokio::test]
async fn new_and_read_only_resumed_sessions_stay_in_the_original_checkout() {
    let h = GitHarness::new().await;
    let original = h.store.list().unwrap()[0].id.clone();
    let base = h.repo.refname_to_id("HEAD").unwrap();
    let index = std::fs::read(h.repo.path().join("index")).unwrap();
    assert!(h.snapshot(&original).worktree.is_none());
    assert_eq!(h.repo.worktrees().unwrap().len(), 0);
    h.read_and_interrupt(&h.workspace.path, "pub fn value() -> u32 { 1 }")
        .await;
    assert!(h.snapshot(&original).worktree.is_none());
    assert_eq!(h.repo.worktrees().unwrap().len(), 0);
    h.command(Command::New).await;
    let fresh = h
        .store
        .list()
        .unwrap()
        .into_iter()
        .find(|snapshot| snapshot.id != original)
        .unwrap();
    assert!(fresh.worktree.is_none());
    h.read_and_interrupt(&h.workspace.path, "pub fn value() -> u32 { 1 }")
        .await;
    assert!(h.snapshot(&fresh.id).worktree.is_none());
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
    assert!(h.snapshot(&original).worktree.is_none());
    h.read_and_interrupt(&h.workspace.path, "pub fn value() -> u32 { 1 }")
        .await;
    assert!(h.snapshot(&original).worktree.is_none());
    assert_eq!(h.repo.worktrees().unwrap().len(), 0);
    for id in [&original, &fresh.id] {
        assert!(
            h.repo
                .find_branch(&format!("joe/session/{id}"), git2::BranchType::Local)
                .is_err()
        );
    }
    assert_eq!(h.repo.refname_to_id("HEAD").unwrap(), base);
    assert_eq!(std::fs::read(h.repo.path().join("index")).unwrap(), index);
    h.stop().await;
}

#[tokio::test]
async fn rejected_first_edits_leave_no_worktree_and_a_valid_retry_is_isolated() {
    let h = GitHarness::new().await;
    let id = h.store.list().unwrap()[0].id.clone();
    let base = h.repo.refname_to_id("HEAD").unwrap();
    let index = std::fs::read(h.repo.path().join("index")).unwrap();
    h.actor
        .send_message(Message::StartWork(Some("Edit safely".into())))
        .unwrap();
    let (_, mut reply) = within(h.requests.recv_async()).await.unwrap();
    for (attempt, patch) in [
        "not a patch",
        "*** Begin Patch\n*** Add File: ../outside.txt\n+denied\n*** End Patch",
        "*** Begin Patch\n*** Update File: lib.rs\n@@\n-missing context\n+replacement\n*** End Patch",
    ].into_iter().enumerate() {
        answer(reply, response(vec![tool_call("apply_patch", &format!("rejected-{attempt}"), json!({"patch":patch}))]));
        let (request, next) = within(h.requests.recv_async()).await.unwrap();
        assert!(matches!(latest_tool_result(&request), ContentBlock::ToolResult { is_error: Some(true), .. }));
        assert!(h.snapshot(&id).worktree.is_none());
        assert_eq!(h.repo.worktrees().unwrap().len(), 0);
        assert!(h.repo.find_branch(&format!("joe/session/{id}"), git2::BranchType::Local).is_err());
        reply = next;
    }
    answer(
        reply,
        response(vec![tool_call(
            "apply_patch",
            "valid",
            json!({"patch":PATCH}),
        )]),
    );
    let (request, reply) = within(h.requests.recv_async()).await.unwrap();
    assert_tool_success(latest_tool_result(&request));
    let worktree = h.snapshot(&id).worktree.unwrap();
    assert_eq!(
        std::fs::read_to_string(worktree.path.join("lib.rs")).unwrap(),
        "pub fn value() -> u32 { 2 }\n"
    );
    assert_eq!(
        std::fs::read_to_string(h.workspace.path.join("lib.rs")).unwrap(),
        "pub fn value() -> u32 { 1 }\n"
    );
    assert_eq!(h.repo.refname_to_id("HEAD").unwrap(), base);
    assert_eq!(std::fs::read(h.repo.path().join("index")).unwrap(), index);
    h.interrupt().await;
    drop(reply);
    h.stop().await;
}

#[tokio::test]
async fn first_write_rejects_a_session_base_that_differs_from_previously_read_files() {
    let h = GitHarness::new().await;
    let id = h.store.list().unwrap()[0].id.clone();
    std::fs::write(
        h.workspace.path.join("lib.rs"),
        "pub fn value() -> u32 { 9 }\n",
    )
    .unwrap();
    h.read_and_interrupt(&h.workspace.path, "pub fn value() -> u32 { 9 }")
        .await;
    h.actor
        .send_message(Message::StartWork(Some("Edit safely".into())))
        .unwrap();
    let (_, reply) = within(h.requests.recv_async()).await.unwrap();
    answer(
        reply,
        response(vec![tool_call(
            "apply_patch",
            "stale",
            json!({"patch":PATCH}),
        )]),
    );
    let (request, reply) = within(h.requests.recv_async()).await.unwrap();
    let result = latest_tool_result(&request);
    assert!(matches!(
        result,
        ContentBlock::ToolResult {
            is_error: Some(true),
            ..
        }
    ));
    assert!(format!("{result:?}").contains("differs from the session base"));
    assert!(h.snapshot(&id).worktree.is_none());
    assert_eq!(h.repo.worktrees().unwrap().len(), 0);
    assert_eq!(
        std::fs::read_to_string(h.workspace.path.join("lib.rs")).unwrap(),
        "pub fn value() -> u32 { 9 }\n"
    );
    h.interrupt().await;
    drop(reply);
    std::fs::write(
        h.workspace.path.join("lib.rs"),
        "pub fn value() -> u32 { 1 }\n",
    )
    .unwrap();
    h.read_and_interrupt(&h.workspace.path, "pub fn value() -> u32 { 1 }")
        .await;
    h.write_and_interrupt(PATCH).await;
    assert!(h.snapshot(&id).worktree.is_some());
    h.stop().await;
}

#[tokio::test]
async fn first_write_creates_one_isolated_worktree_and_later_writes_reuse_it() {
    let h = GitHarness::new().await;
    let id = h.store.list().unwrap()[0].id.clone();
    let base = h.repo.refname_to_id("HEAD").unwrap();
    let index = std::fs::read(h.repo.path().join("index")).unwrap();
    h.actor
        .send_message(Message::StartWork(Some("Update the function".into())))
        .unwrap();
    let (request, reply) = within(h.requests.recv_async()).await.unwrap();
    assert!(
        request.messages[0]
            .text()
            .contains(h.workspace.path.to_str().unwrap())
    );
    assert!(h.snapshot(&id).worktree.is_none());
    assert_eq!(h.repo.worktrees().unwrap().len(), 0);
    answer(
        reply,
        response(vec![tool_call(
            "knowledge",
            "before-write",
            json!({"action":"read", "file_path":"lib.rs"}),
        )]),
    );
    let (request, reply) = within(h.requests.recv_async()).await.unwrap();
    assert_tool_success(latest_tool_result(&request));
    assert!(h.snapshot(&id).worktree.is_none());
    assert_eq!(h.repo.worktrees().unwrap().len(), 0);
    answer(
        reply,
        response(vec![tool_call(
            "apply_patch",
            "first-write",
            json!({"patch": PATCH}),
        )]),
    );
    let (request, reply) = within(h.requests.recv_async()).await.unwrap();
    assert_tool_success(latest_tool_result(&request));
    let worktree = h.snapshot(&id).worktree.unwrap();
    assert_ne!(worktree.path, h.workspace.path);
    assert_eq!(h.repo.worktrees().unwrap().len(), 1);
    assert!(h.repo.find_worktree(&id).is_ok());
    assert!(
        request.messages[0]
            .text()
            .contains(worktree.path.to_str().unwrap())
    );
    assert_eq!(
        std::fs::read_to_string(worktree.path.join("lib.rs")).unwrap(),
        "pub fn value() -> u32 { 2 }\n"
    );
    assert_eq!(
        std::fs::read_to_string(h.workspace.path.join("lib.rs")).unwrap(),
        "pub fn value() -> u32 { 1 }\n"
    );
    answer(
        reply,
        response(vec![tool_call(
            "knowledge",
            "after-write",
            json!({"action":"read", "file_path":"lib.rs"}),
        )]),
    );
    let (request, reply) = within(h.requests.recv_async()).await.unwrap();
    let result = latest_tool_result(&request);
    assert_tool_success(result);
    assert!(format!("{result:?}").contains("pub fn value() -> u32 { 2 }"));
    let patch = "*** Begin Patch\n*** Update File: lib.rs\n@@\n-pub fn value() -> u32 { 2 }\n+pub fn value() -> u32 { 3 }\n*** End Patch";
    answer(
        reply,
        response(vec![tool_call(
            "apply_patch",
            "second-write",
            json!({"patch": patch}),
        )]),
    );
    let (request, reply) = within(h.requests.recv_async()).await.unwrap();
    assert_tool_success(latest_tool_result(&request));
    assert_eq!(h.snapshot(&id).worktree.unwrap().path, worktree.path);
    assert_eq!(h.repo.worktrees().unwrap().len(), 1);
    assert_eq!(
        std::fs::read_to_string(worktree.path.join("lib.rs")).unwrap(),
        "pub fn value() -> u32 { 3 }\n"
    );
    h.interrupt().await;
    drop(reply);
    h.read_and_interrupt(&worktree.path, "pub fn value() -> u32 { 3 }")
        .await;
    assert_eq!(h.snapshot(&id).worktree.unwrap().path, worktree.path);
    assert_eq!(h.repo.worktrees().unwrap().len(), 1);
    assert_eq!(h.repo.refname_to_id("HEAD").unwrap(), base);
    assert_eq!(std::fs::read(h.repo.path().join("index")).unwrap(), index);
    assert_eq!(
        std::fs::read_to_string(h.workspace.path.join("lib.rs")).unwrap(),
        "pub fn value() -> u32 { 1 }\n"
    );
    h.stop().await;
}

#[tokio::test]
async fn plan_handoff_uses_a_fresh_worktree_with_the_planned_files() {
    let h = GitHarness::new().await;
    h.write_and_interrupt(NOOP_PATCH).await;
    let original = h.store.list().unwrap().remove(0);
    let original_worktree = original.worktree.as_ref().unwrap();
    let planned_source = "pub fn value() -> u32 { 7 }\n";
    std::fs::write(original_worktree.path.join("lib.rs"), planned_source).unwrap();
    h.command(Command::Plan).await;
    h.actor
        .send_message(Message::StartWork(Some(
            "Plan a compatible change to value".into(),
        )))
        .unwrap();
    let tool = |name: &str, id: &str, input: Value| {
        let mut block = call(name, id);
        if let ContentBlock::ToolBlock {
            input: arguments, ..
        } = &mut block
        {
            *arguments = input.as_object().unwrap().clone();
        }
        block
    };
    let mut plan = json!({
        "revision":0, "requirements_revision":0,
        "steps":[
            {"id":"inspect", "kind":"investigation", "description":"Inspect the planned files",
             "dependencies":[], "acceptance":"Understand value", "state":"in_progress", "evidence":[], "blocked_reason":null},
            {"id":"implement", "kind":"implementation", "description":"Change value compatibly",
             "dependencies":["inspect"], "acceptance":"The API is unchanged", "state":"pending", "evidence":[], "blocked_reason":null}
        ]
    });
    answer(
        within(h.requests.recv_async()).await.unwrap().1,
        response(vec![
            tool("update_plan", "plan", plan.clone()),
            tool(
                "knowledge",
                "planned-source",
                json!({"action":"read","file_path":"lib.rs"}),
            ),
        ]),
    );
    let (_, reply) = within(h.requests.recv_async()).await.unwrap();
    plan["revision"] = json!(1);
    plan["steps"][0]["state"] = json!("completed");
    plan["steps"][0]["evidence"] =
        json!([{"source":"tool:planned-source", "explanation":"Inspected value returning 7"}]);
    answer(reply, response(vec![tool("update_plan", "planned", plan)]));
    answer(
        within(h.requests.recv_async()).await.unwrap().1,
        response(vec![text(
            "Change value from 7 to 8 without changing its API.",
        )]),
    );
    let packet = h.event(|packet| matches!(packet,
        ActorToTuiPacket::InteractionUpdated(view) if view.questions.iter().any(|question| question.purpose == QuestionPurpose::PlanContinuation)
    )).await;
    let question = match packet {
        ActorToTuiPacket::InteractionUpdated(view) => view.questions[0].clone(),
        _ => panic!("Expected a plan continuation question"),
    };
    let result = h
        .command(Command::Answer(QuestionAnswer {
            id: question.id,
            answer: Answer::Choice {
                choice_id: "new_agent".into(),
            },
        }))
        .await;
    assert!(result.contains("new agent"), "{result}");
    let (request, reply) = within(h.requests.recv_async()).await.unwrap();
    let state = runtime_snapshot(&request.messages);
    assert_eq!(
        state.planning.mode,
        common_models::interaction::WorkMode::Implement
    );
    assert_eq!(state.planning.requirements_revision, 0);
    assert!(state.questions.is_empty());
    let sessions = h.store.list().unwrap();
    assert_eq!(sessions.len(), 2);
    let new = sessions
        .iter()
        .find(|snapshot| snapshot.id != original.id)
        .unwrap();
    assert!(new.worktree.is_none());
    assert!(h.repo.find_worktree(&new.id).is_err());
    assert!(
        request.messages[0]
            .text()
            .contains(&original_worktree.path.display().to_string())
    );
    answer(
        reply,
        response(vec![tool_call(
            "knowledge",
            "handoff-read",
            json!({"action":"read", "file_path":"lib.rs"}),
        )]),
    );
    let (request, reply) = within(h.requests.recv_async()).await.unwrap();
    let result = latest_tool_result(&request);
    assert_tool_success(result);
    assert!(format!("{result:?}").contains("pub fn value() -> u32 { 7 }"));
    assert!(h.snapshot(&new.id).worktree.is_none());
    let patch = "*** Begin Patch\n*** Update File: lib.rs\n@@\n-pub fn value() -> u32 { 7 }\n+pub fn value() -> u32 { 7 }\n*** End Patch";
    answer(
        reply,
        response(vec![tool_call(
            "apply_patch",
            "handoff-write",
            json!({"patch": patch}),
        )]),
    );
    let (request, _reply) = within(h.requests.recv_async()).await.unwrap();
    assert_tool_success(latest_tool_result(&request));
    let worktree = h.snapshot(&new.id).worktree.unwrap();
    assert_ne!(worktree.path, original_worktree.path);
    assert_eq!(
        std::fs::read_to_string(worktree.path.join("lib.rs")).unwrap(),
        planned_source
    );
    assert_eq!(
        std::fs::read_to_string(original_worktree.path.join("lib.rs")).unwrap(),
        planned_source
    );
    assert_eq!(
        std::fs::read_to_string(h.workspace.path.join("lib.rs")).unwrap(),
        "pub fn value() -> u32 { 1 }\n"
    );
    assert!(
        request.messages[0]
            .text()
            .contains(&worktree.path.display().to_string())
    );
    assert!(new.parent.is_none());
    h.stop().await;
}

#[tokio::test]
async fn lazy_fork_sources_survive_prune_and_merge_until_the_fork_writes() {
    let h = GitHarness::new().await;
    let original = h.store.list().unwrap()[0].id.clone();
    let question = h.complete(Some(PATCH)).await;
    h.answer_merge(&question, "skip").await;
    let source = h.snapshot(&original).worktree.unwrap();
    h.command(Command::Fork).await;
    let fork = h
        .store
        .list()
        .unwrap()
        .into_iter()
        .find(|snapshot| snapshot.id != original)
        .unwrap();
    assert!(fork.worktree.is_none());
    assert_eq!(fork.worktree_source.unwrap().path, source.path);
    h.command(Command::New).await;
    let message = h.command(Command::Prune(PruneMode::Force)).await;
    assert!(message.contains("Pruned 0"), "{message}");
    assert!(source.path.exists());
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
    let question = h.complete(None).await;
    let message = h.answer_merge(&question, "merge").await;
    assert!(message.contains("remain recorded"), "{message}");
    assert!(source.path.exists());
    assert_eq!(
        std::fs::read_to_string(h.workspace.path.join("lib.rs")).unwrap(),
        "pub fn value() -> u32 { 2 }\n"
    );
    h.actor
        .send_message(Message::Command(Command::Resume(ResumeTarget::Session {
            id: fork.id.clone(),
        })))
        .unwrap();
    assert!(matches!(
        h.event(|packet| matches!(packet, ActorToTuiPacket::SessionResumed(_)))
            .await,
        ActorToTuiPacket::SessionResumed(Ok(_))
    ));
    h.read_and_interrupt(&source.path, "pub fn value() -> u32 { 2 }")
        .await;
    h.write_and_interrupt("*** Begin Patch\n*** Update File: lib.rs\n@@\n-pub fn value() -> u32 { 2 }\n+pub fn value() -> u32 { 3 }\n*** End Patch").await;
    let fork_tree = h.snapshot(&fork.id).worktree.unwrap();
    assert_ne!(fork_tree.path, source.path);
    assert!(h.snapshot(&fork.id).worktree_source.is_none());
    let message = h.command(Command::Prune(PruneMode::Merged)).await;
    assert!(message.contains("Pruned 1"), "{message}");
    assert!(!source.path.exists());
    assert!(fork_tree.path.exists());
    h.stop().await;
}

#[tokio::test]
async fn prune_removes_inactive_merged_worktrees() {
    let h = GitHarness::new().await;
    h.write_and_interrupt(NOOP_PATCH).await;
    let id = h.store.list().unwrap()[0].id.clone();
    let worktree = h.snapshot(&id).worktree.unwrap();
    h.command(Command::New).await;
    h.write_and_interrupt(NOOP_PATCH).await;
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
    let question = h.complete(Some(PATCH)).await;
    let worktree = h.snapshot(&id).worktree.unwrap();
    let saved = h.snapshot(&id);
    h.command(Command::New).await;
    h.write_and_interrupt(NOOP_PATCH).await;
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
        merge_workflow::MergeApproval::None
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
    assert!(h.snapshot(&id).worktree.is_none());
    assert!(!worktree.path.exists());
    h.read_and_interrupt(&h.workspace.path, "pub fn value() -> u32 { 8 }")
        .await;
    assert!(h.snapshot(&id).worktree.is_none());
    let patch = "*** Begin Patch\n*** Update File: lib.rs\n@@\n-pub fn value() -> u32 { 8 }\n+pub fn value() -> u32 { 8 }\n*** End Patch";
    h.write_and_interrupt(patch).await;
    assert_eq!(h.snapshot(&id).worktree.unwrap().path, worktree.path);
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
    h.write_and_interrupt(NOOP_PATCH).await;
    let project = utils::workspace::WorkspacePolicy::workspace(h.workspace.path.clone()).unwrap();
    let live = h
        .store
        .create(llm::SessionProvider::Injected, None, Vec::new())
        .unwrap();
    let live_worktree =
        utils::git::worktrees::session::SessionWorktree::create(&project, &live.id, None)
            .unwrap()
            .unwrap();
    live.record(session::Event::Worktree(Some(live_worktree.clone())))
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
        .record(session::Event::Worktree(Some(locked_worktree.clone())))
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
    h.complete(Some(PATCH)).await;
    let worktree = h.snapshot(&id).worktree.unwrap();
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
        merge_workflow::MergeApproval::None
    ));
    assert!(h.repo.find_reference(&reference).is_err());
    assert!(
        h.command(Command::Prune(PruneMode::Force))
            .await
            .contains("Pruned 0")
    );
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
    assert!(h.snapshot(&id).worktree.is_none());
    assert!(!worktree.path.exists());
    h.write_and_interrupt(NOOP_PATCH).await;
    assert_eq!(h.snapshot(&id).worktree.unwrap().path, worktree.path);
    assert!(worktree.path.exists());
    h.stop().await;
}

#[tokio::test]
async fn prune_rejects_plan_mode_and_active_turns() {
    let h = GitHarness::new().await;
    h.write_and_interrupt(NOOP_PATCH).await;
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
async fn missing_and_invalid_commit_subjects_are_corrected_in_the_existing_agent_context() {
    let h = GitHarness::new().await;
    h.write_and_interrupt(NOOP_PATCH).await;
    let id = h.store.list().unwrap()[0].id.clone();
    let worktree = h.snapshot(&id).worktree.unwrap();
    let base = h.repo.refname_to_id("HEAD").unwrap();
    h.actor
        .send_message(Message::StartWork(Some(
            "Raise the return value to 2".into(),
        )))
        .unwrap();
    let (_, reply) = within(h.requests.recv_async()).await.unwrap();
    std::fs::write(
        worktree.path.join("lib.rs"),
        "pub fn value() -> u32 { 2 }\n",
    )
    .unwrap();
    answer(
        reply,
        response(vec![call("review_changes", "review-missing-subject")]),
    );
    let (_, reply) = within(h.requests.recv_async()).await.unwrap();
    answer(reply, response(vec![text("Task completed")]));
    let (request, reply) = within(h.requests.recv_async()).await.unwrap();
    assert!(matches!(request.purpose, llm::RequestPurpose::Conversation));
    assert!(!request.tools.is_empty());
    assert!(
        request
            .messages
            .iter()
            .any(|message| message.text().contains("Raise the return value to 2"))
    );
    assert!(request.messages.iter().any(|message| {
        message
            .text()
            .contains("Call review_changes with commit_message")
    }));
    assert!(h.snapshot(&id).questions.pending().is_empty());
    answer(reply, response(vec![review_call("", "invalid-subject")]));
    let (request, reply) = within(h.requests.recv_async()).await.unwrap();
    assert!(matches!(
        latest_tool_result(&request),
        ContentBlock::ToolResult {
            is_error: Some(true),
            ..
        }
    ));
    answer(
        reply,
        response(vec![review_call(
            "Make value return 2 instead of 1",
            "valid-subject",
        )]),
    );
    let (_, reply) = within(h.requests.recv_async()).await.unwrap();
    answer(reply, response(vec![text("Task completed")]));
    let question = h.merge_question().await;
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
        "Make value return 2 instead of 1"
    );
    h.stop().await;
}

#[tokio::test]
async fn large_diffs_use_the_agent_commit_subject_without_a_summary_request() {
    let h = GitHarness::new().await;
    let lines = (0..10_000)
        .map(|index| format!("+Fixture note {index}\n"))
        .collect::<String>();
    let patch = format!("*** Begin Patch\n*** Add File: notes.txt\n{lines}*** End Patch");
    assert!(patch.len() > 64 * 1024);
    let question = h
        .complete_with_subject(Some(&patch), "Add numbered fixture notes")
        .await;
    assert!(
        h.answer_merge(&question, "merge")
            .await
            .contains("Cleaned up")
    );
    assert!(h.requests.is_empty());
    assert_eq!(
        h.repo
            .head()
            .unwrap()
            .peel_to_commit()
            .unwrap()
            .message()
            .unwrap(),
        "Add numbered fixture notes"
    );
    assert!(
        std::fs::metadata(h.workspace.path.join("notes.txt"))
            .unwrap()
            .len()
            > 64 * 1024
    );
    h.stop().await;
}

#[tokio::test]
async fn unchanged_tasks_do_not_request_a_commit_subject_or_merge() {
    let h = GitHarness::new().await;
    let id = h.store.list().unwrap()[0].id.clone();
    let base = h.repo.refname_to_id("HEAD").unwrap();
    assert!(h.snapshot(&id).worktree.is_none());
    assert_eq!(h.repo.worktrees().unwrap().len(), 0);
    h.actor
        .send_message(Message::StartWork(Some("Inspect the function".into())))
        .unwrap();
    let (request, reply) = within(h.requests.recv_async()).await.unwrap();
    assert!(
        request.messages[0]
            .text()
            .contains(h.workspace.path.to_str().unwrap())
    );
    answer(
        reply,
        response(vec![tool_call(
            "knowledge",
            "unchanged-source",
            json!({"action":"read", "file_path":"lib.rs"}),
        )]),
    );
    let (request, reply) = within(h.requests.recv_async()).await.unwrap();
    let result = latest_tool_result(&request);
    assert_tool_success(result);
    assert!(format!("{result:?}").contains("pub fn value() -> u32 { 1 }"));
    assert!(h.snapshot(&id).worktree.is_none());
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
    assert!(h.snapshot(&id).worktree.is_none());
    assert_eq!(h.repo.worktrees().unwrap().len(), 0);
    assert!(h.snapshot(&id).questions.pending().is_empty());
    assert!(matches!(
        h.snapshot(&id).merge_approval,
        merge_workflow::MergeApproval::None
    ));
    h.stop().await;
}

#[tokio::test]
async fn approving_a_conflicted_merge_resolves_and_merges_without_another_question() {
    let h = GitHarness::new().await;
    let id = h.store.list().unwrap()[0].id.clone();
    let question = h.complete(Some(PATCH)).await;
    let worktree = h.snapshot(&id).worktree.unwrap();
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
    let mut read = call("knowledge", "read-conflicts");
    if let ContentBlock::ToolBlock { input, .. } = &mut read {
        *input = json!({"action":"read", "file_path":"lib.rs"})
            .as_object()
            .unwrap()
            .clone();
    }
    answer(reply, response(vec![read]));
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
    answer(
        reply,
        response(vec![review_call(
            "Resolve conflicting return values by returning 5",
            "review-resolution",
        )]),
    );
    let (_, reply) = within(h.requests.recv_async()).await.unwrap();
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
    assert!(h.requests.is_empty());
    assert!(
        std::fs::read_to_string(h.workspace.path.join("lib.rs"))
            .unwrap()
            .contains("{ 5 }")
    );
    assert!(matches!(
        h.snapshot(&id).merge_approval,
        merge_workflow::MergeApproval::None
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
    let question = h.complete(Some(PATCH)).await;
    let worktree = h.snapshot(&id).worktree.unwrap();
    let target = h.commit_main("pub fn value() -> u32 { 3 }\n");
    assert!(
        h.answer_merge(&question, "merge")
            .await
            .contains("Resolving merge conflicts")
    );
    let (_, reply) = within(h.requests.recv_async()).await.unwrap();
    answer(
        reply,
        response(vec![review_call(
            "Resolve conflicting return values",
            "review-conflicts",
        )]),
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
        merge_workflow::MergeApproval::Resolving {
            activity: merge_workflow::ResolutionActivity::Paused,
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
        merge_workflow::MergeApproval::None
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
    let base = h.repo.refname_to_id("HEAD").unwrap();
    let question = h.complete(Some(PATCH)).await;
    let worktree = h.snapshot(&id).worktree.unwrap();
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
        merge_workflow::MergeApproval::None
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
        merge_workflow::MergeApproval::None
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
    use merge_workflow::MergeApproval;
    use session::{Event, ResumableSession};
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
    let question = h.complete(Some(PATCH)).await;
    let worktree = h.snapshot(&original).worktree.unwrap();
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
    assert!(fork.worktree.is_none());
    assert!(h.repo.find_worktree(&fork.id).is_err());
    h.read_and_interrupt(&worktree.path, "pub fn value() -> u32 { 2 }")
        .await;
    assert!(h.snapshot(&fork.id).worktree.is_none());
    let patch = "*** Begin Patch\n*** Update File: lib.rs\n@@\n-pub fn value() -> u32 { 2 }\n+pub fn value() -> u32 { 2 }\n*** End Patch";
    h.write_and_interrupt(patch).await;
    let fork_tree = h.snapshot(&fork.id).worktree.unwrap();
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
        .unwrap();
    assert!(fresh.worktree.is_none());
    h.read_and_interrupt(&h.workspace.path, "pub fn value() -> u32 { 1 }")
        .await;
    assert!(h.snapshot(&fresh.id).worktree.is_none());
    h.write_and_interrupt(NOOP_PATCH).await;
    let fresh = h.snapshot(&fresh.id).worktree.unwrap();
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
        merge_workflow::MergeApproval::None
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
    h.write_and_interrupt(NOOP_PATCH).await;
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
        merge_workflow::MergeApproval::None
    ));
    let merged = h.repo.refname_to_id("HEAD").unwrap();
    let history = h.snapshot(&id).history.len();
    h.read_and_interrupt(&h.workspace.path, "pub fn value() -> u32 { 2 }")
        .await;
    assert!(h.snapshot(&id).worktree.is_none());
    assert!(!worktree.path.exists());
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
    let question = h.complete(Some(PATCH)).await;
    let worktree = h.snapshot(&id).worktree.unwrap();
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
    assert!(h.snapshot(&id).worktree.is_none());
    assert!(!worktree.path.exists());
    h.actor
        .send_message(Message::StartWork(Some(
            "Inspect the resumed workspace".into(),
        )))
        .unwrap();
    let (request, reply) = within(h.requests.recv_async()).await.unwrap();
    assert!(
        request.messages[0]
            .text()
            .contains(h.workspace.path.to_str().unwrap())
    );
    answer(
        reply,
        response(vec![tool_call(
            "knowledge",
            "resumed-source",
            json!({"action":"read", "file_path":"lib.rs"}),
        )]),
    );
    let (request, reply) = within(h.requests.recv_async()).await.unwrap();
    let result = latest_tool_result(&request);
    assert_tool_success(result);
    assert!(format!("{result:?}").contains("pub fn value() -> u32 { 8 }"));
    assert!(h.snapshot(&id).worktree.is_none());
    answer(
        reply,
        response(vec![tool_call("review_changes", "review-main", json!({}))]),
    );
    let (request, reply) = within(h.requests.recv_async()).await.unwrap();
    assert_tool_success(latest_tool_result(&request));
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
    assert!(h.snapshot(&id).worktree.is_none());
    assert!(!worktree.path.exists());
    let patch = "*** Begin Patch\n*** Update File: lib.rs\n@@\n-pub fn value() -> u32 { 8 }\n+pub fn value() -> u32 { 8 }\n*** End Patch";
    h.write_and_interrupt(patch).await;
    assert_eq!(h.snapshot(&id).worktree.unwrap().path, worktree.path);
    assert_eq!(
        git2::Repository::open(&worktree.path)
            .unwrap()
            .refname_to_id("HEAD")
            .unwrap(),
        main
    );
    h.stop().await;
}

#[tokio::test]
async fn cleanup_failure_reports_successful_merge_and_preserves_data_for_retry() {
    let h = GitHarness::new().await;
    h.write_and_interrupt(NOOP_PATCH).await;
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
