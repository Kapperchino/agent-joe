use super::*;
use crate::worker_registry::{
    report::WorkerStatus,
    request::{BudgetLimits, WorkerRequest, WorkerRequestInput},
};

fn worker_input(tools: &str, paths: &str) -> Value {
    json!({
        "objective": "Inspect the assigned files and report evidence",
        "constraints": "Keep public APIs stable",
        "allowed_tools": tools,
        "allowed_paths": paths,
        "context": "Selected context marker",
        "completion_criteria": "Report findings and checks actually run"
    })
}

async fn interaction_command(
    actor: &RepositoryActor,
    command: commands::command::Command,
) -> String {
    actor
        .actor
        .send_message(Message::Command(command.clone()))
        .unwrap();
    let event = actor.event(|event| event.actor_id == 0 && matches!(&event.packet, ActorToTuiPacket::CommandResult(found, _) if *found == command)).await;
    match event.packet {
        ActorToTuiPacket::CommandResult(_, text) => text,
        _ => panic!("Expected command result"),
    }
}

#[tokio::test]
async fn plan_mode_denies_all_cargo_and_mutation_tools_in_both_root_modes() {
    use commands::command::Command;
    for mode in [Mode::Simple, Mode::Delegated] {
        let workspace = crate::session::tests::Workspace::new();
        std::fs::write(workspace.path.join("original.txt"), "user work").unwrap();
        let actor = match mode {
            Mode::Simple => RepositoryActor::new(SimpleWorker::new(), workspace.path.clone()).await,
            Mode::Delegated => {
                RepositoryActor::new(BaseWorker::new(), workspace.path.clone()).await
            }
        };
        assert!(
            interaction_command(&actor, Command::Plan)
                .await
                .contains("Plan mode")
        );
        assert!(
            interaction_command(&actor, Command::Undo("missing".into()))
                .await
                .contains("Plan mode")
        );
        actor
            .actor
            .send_message(Message::StartWork(Some("Investigate safely".into())))
            .unwrap();
        let (request, reply) = actor.request().await;
        assert!(request.system.unwrap().contains("Work mode: Plan"));
        assert!(request.tools.iter().any(
            |tool| matches!(tool, ToolDefinition::Client { name, .. } if name == "update_plan")
        ));
        let mutations = [
            tool(
                "apply_patch",
                "patch",
                json!({"patch":"*** Begin Patch\n*** Add File: forbidden.txt\n+changed\n*** End Patch"}),
            ),
            tool("undo_changes", "undo", json!({"edit_id":"missing"})),
            tool(
                "worktree",
                "create",
                json!({"operation":"create","base":"HEAD"}),
            ),
            tool(
                "worktree",
                "integrate",
                json!({"operation":"integrate","id":"missing"}),
            ),
            tool(
                "worktree",
                "remove",
                json!({"operation":"remove","id":"missing"}),
            ),
        ];
        let cargo = [
            "check",
            "test",
            "fmt",
            "fmt_check",
            "clippy",
            "run",
            "start",
            "poll",
            "stop",
        ]
        .into_iter()
        .map(|operation| {
            let input = match operation {
                "run" | "start" => {
                    json!({"operation":operation,"target":{"kind":"bin","name":"app"}})
                }
                "poll" | "stop" => json!({"operation":operation,"process_id":"missing"}),
                _ => json!({"operation":operation}),
            };
            tool("cargo", operation, input)
        });
        answer(
            reply,
            response(mutations.into_iter().chain(cargo).collect()),
        );
        let (request, reply) = actor.request().await;
        let results = request
            .messages
            .iter()
            .flat_map(|message| &message.content)
            .filter_map(|block| match block {
                ContentBlock::ToolResult {
                    content, is_error, ..
                } => Some((content, is_error)),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(results.len(), 14);
        for (content, is_error) in results {
            assert_eq!(*is_error, Some(true));
            assert!(content.contains("Plan mode"), "{content}");
        }
        assert!(!workspace.path.join("forbidden.txt").exists());
        assert!(!workspace.path.join("target").exists());
        assert_eq!(
            std::fs::read_to_string(workspace.path.join("original.txt")).unwrap(),
            "user work"
        );
        answer(
            reply,
            response(vec![
                tool("read_file", "inspect", json!({"file_path":"original.txt"})),
                tool("worktree", "list", json!({"operation":"list"})),
            ]),
        );
        let (request, reply) = actor.request().await;
        assert!(result_text(&request).contains("user work"));
        completed_root(&actor, reply).await;
        interaction_command(&actor, Command::Implement).await;
        actor
            .actor
            .send_message(Message::StartWork(Some("Create the file".into())))
            .unwrap();
        answer(
            actor.request().await.1,
            response(vec![tool(
                "apply_patch",
                "allowed",
                json!({"patch":"*** Begin Patch\n*** Add File: allowed.txt\n+implemented\n*** End Patch"}),
            )]),
        );
        let (_, reply) = actor.request().await;
        assert_eq!(
            std::fs::read_to_string(workspace.path.join("allowed.txt")).unwrap(),
            "implemented"
        );
        completed_root(&actor, reply).await;
        actor.stop().await;
    }
}

#[tokio::test]
async fn plan_mode_is_inherited_by_read_workers_and_denies_dynamic_writer_launches() {
    use commands::command::Command;
    let workspace = crate::session::tests::Workspace::new();
    let actor = RepositoryActor::new(BaseWorker::new(), workspace.path.clone()).await;
    interaction_command(&actor, Command::Plan).await;
    let started = StartedWorker::new(&actor, worker_input("find_files", ".")).await;
    assert!(
        started
            .child
            .0
            .system
            .as_ref()
            .unwrap()
            .contains("Work mode: Plan")
    );
    assert!(!started.child.0.tools.iter().any(
        |tool| matches!(tool, ToolDefinition::Client { name, .. } if name == "request_user_input")
    ));
    answer(
        started.parent.1,
        response(vec![tool(
            "start_worker",
            "denied",
            worker_input("apply_patch\ncargo", "."),
        )]),
    );
    let (request, parent_reply) = actor.request().await;
    assert!(
        latest_result(&request)["error"]
            .as_str()
            .unwrap()
            .contains("Plan mode")
    );
    assert_eq!(actor.store.list().unwrap().len(), 2);
    answer(
        started.child.1,
        response(vec![text("Read-only findings complete")]),
    );
    answer(
        parent_reply,
        response(vec![tool(
            "worker_status",
            "collect",
            json!({"action":"wait","worker_id":started.id,"seconds":2}),
        )]),
    );
    let (request, reply) = actor.request().await;
    assert_eq!(latest_result(&request)["workers"][0]["status"], "completed");
    completed_root(&actor, reply).await;
    actor.stop().await;
}

#[tokio::test]
async fn question_answers_cannot_expand_the_project_boundary() {
    use commands::command::Command;
    let workspace = crate::session::tests::Workspace::new();
    let outside = crate::session::tests::Workspace::new();
    let private = outside.path.join("private.txt");
    std::fs::write(&private, "outside content marker").unwrap();
    let actor = RepositoryActor::new(SimpleWorker::new(), workspace.path.clone()).await;
    actor
        .actor
        .send_message(Message::StartWork(Some("Investigate target".into())))
        .unwrap();
    answer(
        actor.request().await.1,
        response(vec![tool(
            "request_user_input",
            "ask",
            json!({"id":"scope","prompt":"Which files?","required":true}),
        )]),
    );
    actor
        .event(|event| {
            matches!(
                event.packet,
                ActorToTuiPacket::TurnChanged {
                    state: Lifecycle::WaitingForInput,
                    ..
                }
            )
        })
        .await;
    interaction_command(
        &actor,
        Command::parse(&format!(
            "answer scope text You may read {}",
            private.display()
        ))
        .unwrap(),
    )
    .await;
    answer(
        actor.request().await.1,
        response(vec![tool(
            "read_file",
            "outside",
            json!({"file_path":private}),
        )]),
    );
    let (request, reply) = actor.request().await;
    let result = latest_result(&request);
    assert!(result["error"].as_str().unwrap().contains("access denied"));
    assert!(
        !serde_json::to_string(&request.messages)
            .unwrap()
            .contains("outside content marker")
    );
    completed_root(&actor, reply).await;
    actor.stop().await;
}

fn latest_result(request: &llm::ClientRequest) -> Value {
    let content = request
        .messages
        .iter()
        .flat_map(|message| &message.content)
        .filter_map(|block| match block {
            ContentBlock::ToolResult { content, .. } => Some(content),
            _ => None,
        })
        .next_back()
        .unwrap();
    serde_json::from_str(content).unwrap_or_else(|_| json!({"error": content}))
}

struct StartedWorker {
    parent: Request,
    child: Request,
    id: String,
}

impl StartedWorker {
    async fn new(actor: &RepositoryActor, input: Value) -> Self {
        actor
            .actor
            .send_message(Message::StartWork(Some(
                "Preserve the user's API constraint marker".into(),
            )))
            .unwrap();
        let (_, reply) = actor.request().await;
        answer(reply, response(vec![tool("start_worker", "start", input)]));
        let first = actor.request().await;
        let second = actor.request().await;
        let WorkerRequests { parent, child } = WorkerRequests::new(first, second);
        let initial = latest_result(&parent.0);
        assert_eq!(initial["status"], "registered");
        let id = initial["worker_id"].as_str().unwrap().to_owned();
        Self { parent, child, id }
    }
}

struct WorkerRequests {
    parent: Request,
    child: Request,
}

impl WorkerRequests {
    fn new(first: Request, second: Request) -> Self {
        match first
            .0
            .messages
            .iter()
            .any(|message| message.text().contains("Bounded worker request:"))
        {
            true => Self {
                parent: second,
                child: first,
            },
            false => Self {
                parent: first,
                child: second,
            },
        }
    }
}

async fn completed_root(actor: &RepositoryActor, reply: oneshot::Sender<anyhow::Result<Events>>) {
    answer(
        reply,
        response(vec![text("Finished and reviewed worker evidence.")]),
    );
    actor
        .event(|event| {
            event.actor_id == 0
                && matches!(
                    event.packet,
                    ActorToTuiPacket::TurnChanged {
                        state: Lifecycle::Completed,
                        ..
                    }
                )
        })
        .await;
}

#[tokio::test]
async fn root_modes_complete_small_changes_directly_and_simple_has_no_delegation() {
    for mode in [Mode::Simple, Mode::Delegated] {
        let workspace = crate::session::tests::Workspace::new();
        let actor = match mode {
            Mode::Simple => RepositoryActor::new(SimpleWorker::new(), workspace.path.clone()).await,
            Mode::Delegated => {
                RepositoryActor::new(BaseWorker::new(), workspace.path.clone()).await
            }
        };
        actor
            .actor
            .send_message(Message::StartWork(Some("Create a small file".into())))
            .unwrap();
        let (request, reply) = actor.request().await;
        let tools = request
            .tools
            .iter()
            .filter_map(|tool| match tool {
                ToolDefinition::Client { name, .. } => Some(name.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(
            tools.contains(&"read_file")
                && tools.contains(&"apply_patch")
                && tools.contains(&"cargo")
        );
        assert_eq!(
            tools.contains(&"start_worker"),
            matches!(mode, Mode::Delegated)
        );
        answer(
            reply,
            response(vec![tool(
                "apply_patch",
                "edit",
                json!({"patch": "*** Begin Patch\n*** Add File: new.txt\n+direct\n*** End Patch"}),
            )]),
        );
        let (_, reply) = actor.request().await;
        assert_eq!(
            std::fs::read_to_string(workspace.path.join("new.txt")).unwrap(),
            "direct"
        );
        completed_root(&actor, reply).await;
        assert_eq!(actor.store.list().unwrap().len(), 1);
        actor.stop().await;
    }
}

#[tokio::test]
async fn bounded_worker_inherits_constraints_denies_other_paths_and_returns_observed_changes() {
    let workspace = crate::session::tests::Workspace::new();
    std::fs::create_dir_all(workspace.path.join("assigned")).unwrap();
    std::fs::write(workspace.path.join("secret.txt"), "secret content").unwrap();
    std::fs::write(
        workspace.path.join("assigned/evidence.txt"),
        "artifact evidence\n".repeat(1500),
    )
    .unwrap();
    std::fs::create_dir_all(workspace.path.join("logs")).unwrap();
    let actor = RepositoryActor::configured(BaseWorker::new(), workspace.path.clone(), true).await;
    let started =
        StartedWorker::new(&actor, worker_input("read_file\napply_patch", "assigned")).await;
    let worker_request = &started.child.0;
    let handoff = worker_request
        .messages
        .iter()
        .map(llm::Message::text)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        handoff.contains("user's API constraint marker")
            && handoff.contains("Keep public APIs stable")
            && handoff.contains("Selected context marker")
    );
    assert_eq!(worker_request.tools.len(), 2);
    assert!(worker_request.max_output_tokens.unwrap() <= 4096);
    answer(
        started.child.1,
        response(vec![tool(
            "read_file",
            "denied",
            json!({"file_path": "secret.txt"}),
        )]),
    );
    let (denied, reply) = actor.request().await;
    assert!(result_text(&denied).contains("Worker path access denied"));
    assert!(!result_text(&denied).contains("secret content"));
    answer(
        reply,
        response(vec![tool(
            "apply_patch",
            "allowed",
            json!({"patch":"*** Begin Patch\n*** Add File: assigned/result.txt\n+bounded\n*** End Patch"}),
        )]),
    );
    let (_, reply) = actor.request().await;
    answer(
        reply,
        response(vec![tool(
            "read_file",
            "large-evidence",
            json!({"file_path":"assigned/evidence.txt"}),
        )]),
    );
    let (archived, reply) = actor.request().await;
    assert!(result_text(&archived).contains("Full output: artifact"));
    answer(reply, response(vec![text("Created the assigned result.")]));
    answer(
        started.parent.1,
        response(vec![tool(
            "worker_status",
            "wait",
            json!({"action":"wait", "worker_id":started.id, "seconds":2}),
        )]),
    );
    let (parent, reply) = actor.request().await;
    let result = latest_result(&parent);
    let report = &result["workers"][0]["report"];
    assert_eq!(report["status"], "completed");
    assert_eq!(report["changed_files"], json!(["assigned/result.txt"]));
    assert!(
        report["unresolved_issues"]
            .as_array()
            .unwrap()
            .iter()
            .any(|issue| issue.as_str().unwrap().contains("No validation checks"))
    );
    assert_eq!(report["budget"]["tool_calls"], 3);
    assert_eq!(report["artifacts"].as_array().unwrap().len(), 1);
    let artifact = report["artifacts"][0]["id"].as_str().unwrap();
    answer(
        reply,
        response(vec![tool(
            "read_artifact",
            "parent-evidence",
            json!({"id":artifact, "offset":0, "bytes":512}),
        )]),
    );
    let (retrieved, reply) = actor.request().await;
    assert!(result_text(&retrieved).contains("artifact evidence"));
    completed_root(&actor, reply).await;
    let snapshots = actor.store.list().unwrap();
    let root = snapshots
        .iter()
        .find(|snapshot| snapshot.parent.is_none())
        .unwrap();
    assert_eq!(
        root.workers[&started.id]
            .report
            .as_ref()
            .unwrap()
            .changed_files,
        vec!["assigned/result.txt"]
    );
    assert_eq!(
        std::fs::read_to_string(workspace.path.join("secret.txt")).unwrap(),
        "secret content"
    );
    actor.stop().await;
}

#[tokio::test]
async fn writer_ownership_rejects_overlapping_workers_and_root_edits_until_cleanup() {
    let workspace = crate::session::tests::Workspace::new();
    let actor = RepositoryActor::new(BaseWorker::new(), workspace.path.clone()).await;
    let started = StartedWorker::new(&actor, worker_input("apply_patch", ".")).await;
    answer(
        started.parent.1,
        response(vec![tool(
            "start_worker",
            "overlap",
            worker_input("apply_patch", "."),
        )]),
    );
    let (rejected, reply) = actor.request().await;
    assert!(result_text(&rejected).contains("writer already owns"));
    answer(
        reply,
        response(vec![tool(
            "apply_patch",
            "root-write",
            json!({"patch":"*** Begin Patch\n*** Add File: forbidden.txt\n+overlap\n*** End Patch"}),
        )]),
    );
    let (rejected, reply) = actor.request().await;
    assert!(result_text(&rejected).contains("worker owns workspace writes"));
    assert!(!workspace.path.join("forbidden.txt").exists());
    answer(
        reply,
        response(vec![tool(
            "worker_status",
            "cancel",
            json!({"action":"cancel", "worker_id":started.id}),
        )]),
    );
    let (_, reply) = actor.request().await;
    answer(
        reply,
        response(vec![tool(
            "worker_status",
            "wait",
            json!({"action":"wait", "worker_id":started.id, "seconds":2}),
        )]),
    );
    let (cancelled, reply) = actor.request().await;
    assert_eq!(
        latest_result(&cancelled)["workers"][0]["status"],
        "cancelled"
    );
    assert!(started.child.1.is_closed());
    answer(
        reply,
        response(vec![tool(
            "apply_patch",
            "root-after-cleanup",
            json!({"patch":"*** Begin Patch\n*** Add File: allowed.txt\n+after cleanup\n*** End Patch"}),
        )]),
    );
    let (_, reply) = actor.request().await;
    assert!(workspace.path.join("allowed.txt").exists());
    completed_root(&actor, reply).await;
    actor.stop().await;
}

#[tokio::test]
async fn timeout_failure_and_request_budget_are_reported_with_cleanup() {
    for failure in [
        WorkerStatus::TimedOut,
        WorkerStatus::Failed,
        WorkerStatus::BudgetExhausted,
    ] {
        let workspace = crate::session::tests::Workspace::new();
        let actor = RepositoryActor::new(BaseWorker::new(), workspace.path.clone()).await;
        let mut input = worker_input("find_files", ".");
        input["seconds"] = json!(1);
        input["requests"] = json!(1);
        let started = StartedWorker::new(&actor, input).await;
        match failure {
            WorkerStatus::Failed => {
                assert!(
                    started
                        .child
                        .1
                        .send(Err(Failure::new(
                            FailureKind::InvalidInput,
                            "child failure marker"
                        )
                        .into()))
                        .is_ok()
                );
            }
            WorkerStatus::BudgetExhausted => answer(
                started.child.1,
                response(vec![tool("find_files", "read", json!({"pattern":""}))]),
            ),
            _ => {}
        }
        answer(
            started.parent.1,
            response(vec![tool(
                "worker_status",
                "wait",
                json!({"action":"wait", "worker_id":started.id, "seconds":2}),
            )]),
        );
        let (parent, reply) = actor.request().await;
        let expected = serde_json::to_value(failure).unwrap();
        assert_eq!(latest_result(&parent)["workers"][0]["status"], expected);
        completed_root(&actor, reply).await;
        actor.stop().await;
    }
}

#[tokio::test]
async fn parent_interrupt_cancels_and_journals_children_without_replaying_them() {
    let workspace = crate::session::tests::Workspace::new();
    let actor = RepositoryActor::new(BaseWorker::new(), workspace.path.clone()).await;
    let started = StartedWorker::new(&actor, worker_input("find_files", ".")).await;
    actor.actor.send_message(Message::Interrupt).unwrap();
    actor
        .event(|event| {
            event.actor_id == 0
                && matches!(
                    event.packet,
                    ActorToTuiPacket::TurnChanged {
                        state: Lifecycle::Cancelled,
                        ..
                    }
                )
        })
        .await;
    assert!(started.parent.1.is_closed() && started.child.1.is_closed());
    let snapshots = actor.store.list().unwrap();
    let root = snapshots
        .iter()
        .find(|snapshot| snapshot.parent.is_none())
        .unwrap();
    assert_eq!(root.workers[&started.id].status, WorkerStatus::Cancelled);
    let id = root.id.clone();
    actor.stop().await;
    let policy = utils::workspace::WorkspacePolicy::workspace(workspace.path.clone()).unwrap();
    let resumed = crate::session::ResumableSession::new(
        &actor_store(&workspace.path),
        &id,
        &policy,
        &llm::SessionProvider::Injected,
    );
    assert!(resumed.is_ok());
}

fn actor_store(path: &std::path::Path) -> Arc<crate::session::SessionStore> {
    Runtime::for_workspace(path.to_path_buf())
        .unwrap()
        .sessions
        .unwrap()
}

#[test]
fn worker_contracts_and_conservative_budgets_reject_unbounded_or_widened_requests() {
    let valid =
        || serde_json::from_value::<WorkerRequestInput>(worker_input("read_file", "src")).unwrap();
    for field in ["tokens", "seconds", "requests"] {
        let mut input = worker_input("read_file", "src");
        input[field] = json!(0);
        assert!(
            WorkerRequest::new(serde_json::from_value(input).unwrap(), |_| Some(
                ToolEffect::Read
            ))
            .is_err()
        );
    }
    assert!(WorkerRequest::new(valid(), |_| Some(ToolEffect::DelegateRead)).is_err());
    assert!(WorkerRequest::new(valid(), |_| None).is_err());
    let mut input = valid();
    input.allowed_paths = "../outside".into();
    assert!(WorkerRequest::new(input, |_| Some(ToolEffect::Read)).is_err());
    let budget =
        crate::worker_registry::budget::WorkerBudget::new(BudgetLimits::new(4096, 1, 1).unwrap());
    let mut request = llm::ClientRequest::new(vec![llm::Message::new("small request".into())]);
    budget.reserve(&mut request).unwrap();
    assert!(request.max_output_tokens.unwrap() <= 4096);
    assert!(budget.reserve(&mut request).is_err());
    assert_eq!(
        budget.usage().state,
        crate::worker_registry::budget::BudgetState::Exhausted
    );
}

#[tokio::test]
async fn independent_read_workers_run_concurrently_and_followups_receive_only_selected_reports() {
    let workspace = crate::session::tests::Workspace::new();
    let actor = RepositoryActor::new(BaseWorker::new(), workspace.path.clone()).await;
    actor
        .actor
        .send_message(Message::StartWork(Some(
            "Keep parent constraint marker".into(),
        )))
        .unwrap();
    let (_, reply) = actor.request().await;
    answer(
        reply,
        response(vec![
            tool("start_worker", "one", worker_input("find_files", ".")),
            tool("start_worker", "two", worker_input("read_file", ".")),
        ]),
    );
    let mut parent = None;
    let mut children = Vec::new();
    for _ in 0..3 {
        let request = actor.request().await;
        match request
            .0
            .messages
            .iter()
            .any(|message| message.text().contains("Bounded worker request:"))
        {
            true => children.push(request),
            false => parent = Some(request),
        }
    }
    assert_eq!(children.len(), 2);
    let (parent, reply) = parent.unwrap();
    let ids = parent
        .messages
        .iter()
        .flat_map(|message| &message.content)
        .filter_map(|block| match block {
            ContentBlock::ToolResult { content, .. } => serde_json::from_str::<Value>(content)
                .ok()
                .and_then(|value| value["worker_id"].as_str().map(str::to_owned)),
            _ => None,
        })
        .collect::<Vec<_>>();
    for (request, child_reply) in children {
        let marker =
            match request.tools.iter().any(
                |tool| matches!(tool, ToolDefinition::Client { name, .. } if name == "find_files"),
            ) {
                true => "Selected investigation marker",
                false => "Unrelated investigation marker",
            };
        answer(child_reply, response(vec![text(marker)]));
    }
    answer(
        reply,
        response(
            ids.iter()
                .enumerate()
                .map(|(index, id)| {
                    tool(
                        "worker_status",
                        &format!("wait-{index}"),
                        json!({"action":"wait", "worker_id":id, "seconds":2}),
                    )
                })
                .collect(),
        ),
    );
    let (_, reply) = actor.request().await;
    answer(
        reply,
        response(vec![tool(
            "worker_status",
            "follow",
            json!({"action":"follow_up", "worker_id":ids[0], "message":"Confirm the selected finding"}),
        )]),
    );
    let first = actor.request().await;
    let second = actor.request().await;
    let WorkerRequests { parent, child } = WorkerRequests::new(first, second);
    let handoff = child
        .0
        .messages
        .iter()
        .map(llm::Message::text)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(handoff.contains("Selected investigation marker"));
    assert!(!handoff.contains("Unrelated investigation marker"));
    assert!(handoff.contains("Keep parent constraint marker"));
    let next_id = latest_result(&parent.0)["workers"][0]["worker_id"]
        .as_str()
        .unwrap()
        .to_owned();
    answer(child.1, response(vec![text("Confirmed selected finding")]));
    answer(
        parent.1,
        response(vec![tool(
            "worker_status",
            "wait-follow",
            json!({"action":"wait", "worker_id":next_id, "seconds":2}),
        )]),
    );
    let (_, reply) = actor.request().await;
    completed_root(&actor, reply).await;
    actor.stop().await;
}

#[tokio::test]
async fn uncollected_worker_reports_prevent_silent_parent_completion() {
    let workspace = crate::session::tests::Workspace::new();
    let actor = RepositoryActor::new(BaseWorker::new(), workspace.path.clone()).await;
    let started = StartedWorker::new(&actor, worker_input("find_files", ".")).await;
    answer(
        started.parent.1,
        response(vec![text(
            "Claiming completion without collecting worker evidence",
        )]),
    );
    actor
        .event(|event| {
            event.actor_id == 0
                && matches!(
                    event.packet,
                    ActorToTuiPacket::TurnChanged {
                        state: Lifecycle::Failed,
                        ..
                    }
                )
        })
        .await;
    assert!(started.child.1.is_closed());
    actor.stop().await;
}

#[tokio::test]
async fn worker_reports_preserve_actual_validation_failure_and_original_parameters() {
    let workspace = crate::session::tests::Workspace::new();
    let actor = RepositoryActor::new(BaseWorker::new(), workspace.path.clone()).await;
    let started = StartedWorker::new(&actor, worker_input("cargo", ".")).await;
    answer(
        started.child.1,
        response(vec![tool(
            "cargo",
            "invalid-selector",
            json!({"operation":"test", "package":"--injected", "test_name":null}),
        )]),
    );
    let (child, reply) = actor.request().await;
    assert!(result_text(&child).contains("Invalid Cargo selector"));
    answer(
        reply,
        response(vec![text("The requested check could not start")]),
    );
    answer(
        started.parent.1,
        response(vec![tool(
            "worker_status",
            "wait",
            json!({"action":"wait", "worker_id":started.id, "seconds":2}),
        )]),
    );
    let (parent, reply) = actor.request().await;
    let result = latest_result(&parent);
    let report = &result["workers"][0]["report"];
    assert_eq!(report["validation"][0]["invocation"]["name"], "cargo");
    assert_eq!(
        report["validation"][0]["invocation"]["input"]["package"],
        "--injected"
    );
    assert!(
        report["validation"][0]["outcome"]["Err"]["message"]
            .as_str()
            .unwrap()
            .contains("Invalid Cargo selector")
    );
    assert!(!report["unresolved_issues"].as_array().unwrap().is_empty());
    completed_root(&actor, reply).await;
    actor.stop().await;
}

#[tokio::test]
async fn scoped_workers_reject_whole_workspace_cargo_tools_before_startup() {
    let workspace = crate::session::tests::Workspace::new();
    std::fs::create_dir(workspace.path.join("assigned")).unwrap();
    let actor = RepositoryActor::new(BaseWorker::new(), workspace.path.clone()).await;
    actor
        .actor
        .send_message(Message::StartWork(Some(
            "Check scoped worker permissions".into(),
        )))
        .unwrap();
    let (_, mut reply) = actor.request().await;
    for name in ["cargo", "git", "review_changes", "worktree"] {
        answer(
            reply,
            response(vec![tool(
                "start_worker",
                name,
                worker_input(name, "assigned"),
            )]),
        );
        let (rejected, next) = actor.request().await;
        assert!(result_text(&rejected).contains("require whole-project paths"));
        reply = next;
    }
    completed_root(&actor, reply).await;
    assert_eq!(actor.store.list().unwrap().len(), 1);
    actor.stop().await;
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[tokio::test]
async fn worker_cancellation_drains_managed_targets_and_reports_final_process_evidence() {
    if utils::test_support::sandbox_available() {
        let workspace = crate::session::tests::Workspace::new();
        std::fs::create_dir(workspace.path.join("examples")).unwrap();
        std::fs::write(
            workspace.path.join("Cargo.toml"),
            "[package]\nname = 'worker_process_fixture'\nversion = '0.1.0'\nedition = '2024'\n",
        )
        .unwrap();
        std::fs::write(workspace.path.join("examples/server.rs"), "fn main() {\n    use std::io::Write;\n    println!(\"worker-ready\");\n    std::io::stdout().flush().unwrap();\n    std::fs::write(\"ready\", \"ready\").unwrap();\n    loop { std::thread::sleep(std::time::Duration::from_millis(50)); }\n}\n").unwrap();
        let actor = RepositoryActor::new(BaseWorker::new(), workspace.path.clone()).await;
        let started = StartedWorker::new(&actor, worker_input("cargo", ".")).await;
        answer(
            started.child.1,
            response(vec![tool(
                "cargo",
                "start-target",
                json!({"operation":"start", "target":{"kind":"example","name":"server"}}),
            )]),
        );
        let (child, child_reply) = actor.request().await;
        let process = latest_cargo(&child);
        assert_eq!(process.status, utils::process::ProcessStatus::Running);
        let process_id = process.process_id.unwrap();
        tokio::time::timeout(Duration::from_secs(20), async {
            while !workspace.path.join("ready").exists() {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        answer(
            started.parent.1,
            response(vec![tool(
                "worker_status",
                "cancel-worker",
                json!({"action":"cancel", "worker_id":started.id}),
            )]),
        );
        let (_, reply) = actor.request().await;
        answer(
            reply,
            response(vec![tool(
                "worker_status",
                "wait-worker",
                json!({"action":"wait", "worker_id":started.id, "seconds":20}),
            )]),
        );
        let (parent, reply) = actor.request().await;
        let result = latest_result(&parent);
        let report = &result["workers"][0]["report"];
        assert_eq!(report["status"], "cancelled");
        assert_eq!(report["processes"][0]["process_id"], process_id);
        assert_eq!(report["processes"][0]["status"], "cancelled");
        assert!(
            report["processes"][0]["stdout"]["content"]
                .as_str()
                .unwrap()
                .contains("worker-ready")
        );
        assert_eq!(report["validation"][0]["invocation"]["name"], "cargo");
        assert!(child_reply.is_closed());
        completed_root(&actor, reply).await;
        actor.stop().await;
    }
}

#[tokio::test]
async fn worker_edits_share_parent_journal_and_exclude_root_undo_and_worktree_writes() {
    let workspace = crate::session::tests::Workspace::new();
    std::fs::create_dir(workspace.path.join("assigned")).unwrap();
    std::fs::write(workspace.path.join("existing.txt"), "user baseline").unwrap();
    let actor = RepositoryActor::new(BaseWorker::new(), workspace.path.clone()).await;
    let started = StartedWorker::new(&actor, worker_input("apply_patch", "assigned")).await;
    answer(
        started.child.1,
        response(vec![tool(
            "apply_patch",
            "worker-edit",
            json!({"patch":"*** Begin Patch\n*** Add File: assigned/new.txt\n+worker edit\n*** End Patch"}),
        )]),
    );
    let (child, child_reply) = actor.request().await;
    let edit_id = latest_result(&child)["edit"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    answer(
        started.parent.1,
        response(vec![tool(
            "undo_changes",
            "blocked-undo",
            json!({"edit_id":edit_id}),
        )]),
    );
    let (rejected, reply) = actor.request().await;
    assert!(result_text(&rejected).contains("worker owns workspace writes"));
    answer(
        reply,
        response(vec![tool(
            "worktree",
            "blocked-worktree",
            json!({"operation":"create", "base":"HEAD"}),
        )]),
    );
    let (rejected, reply) = actor.request().await;
    assert!(result_text(&rejected).contains("worker owns workspace writes"));
    answer(
        child_reply,
        response(vec![text(
            "Created the assigned file; parent review remains",
        )]),
    );
    answer(
        reply,
        response(vec![tool(
            "worker_status",
            "wait",
            json!({"action":"wait", "worker_id":started.id, "seconds":2}),
        )]),
    );
    let (parent, reply) = actor.request().await;
    assert_eq!(
        latest_result(&parent)["workers"][0]["report"]["edits"][0]["id"],
        edit_id
    );
    answer(
        reply,
        response(vec![tool("review_changes", "parent-review", json!({}))]),
    );
    let (reviewed, reply) = actor.request().await;
    let review = latest_result(&reviewed);
    assert_eq!(review["changes"][0]["path"], "assigned/new.txt");
    assert_eq!(review["changes"][0]["ownership"], "joe");
    answer(
        reply,
        response(vec![tool(
            "undo_changes",
            "parent-undo",
            json!({"edit_id":edit_id}),
        )]),
    );
    let (undone, reply) = actor.request().await;
    assert_eq!(latest_result(&undone)["undo_of"], edit_id);
    assert!(!workspace.path.join("assigned/new.txt").exists());
    assert_eq!(
        std::fs::read_to_string(workspace.path.join("existing.txt")).unwrap(),
        "user baseline"
    );
    let snapshots = actor.store.list().unwrap();
    let root = snapshots
        .iter()
        .find(|snapshot| snapshot.parent.is_none())
        .unwrap();
    assert_eq!(root.changes.records.len(), 2);
    assert_eq!(root.changes.records[0].id, edit_id);
    assert!(
        root.changes
            .baseline
            .as_ref()
            .unwrap()
            .files
            .contains_key(std::path::Path::new("existing.txt"))
    );
    completed_root(&actor, reply).await;
    actor.stop().await;
}
