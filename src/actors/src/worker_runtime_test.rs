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
                && tools.contains(&"cargo_test")
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
    let started = StartedWorker::new(&actor, worker_input("cargo_test", ".")).await;
    answer(
        started.child.1,
        response(vec![tool(
            "cargo_test",
            "invalid-selector",
            json!({"package":"--injected", "test_name":null}),
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
    assert_eq!(report["validation"][0]["invocation"]["name"], "cargo_test");
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
    for name in ["cargo_check", "cargo_fmt", "cargo_run", "cargo_start"] {
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
        let started = StartedWorker::new(
            &actor,
            worker_input("cargo_start\nprocess_poll\nprocess_stop", "."),
        )
        .await;
        answer(
            started.child.1,
            response(vec![tool(
                "cargo_start",
                "start-target",
                json!({"target":{"kind":"example","name":"server"}}),
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
        assert_eq!(report["validation"][0]["invocation"]["name"], "cargo_start");
        assert!(child_reply.is_closed());
        completed_root(&actor, reply).await;
        actor.stop().await;
    }
}
