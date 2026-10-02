use super::*;

fn workflow_input(tools: &str) -> Value {
    json!({
        "context":"Shared workflow requirement",
        "agents":[
            {"name":"coder", "instructions":"Code specialization marker", "allowed_tools":tools, "allowed_paths":".", "seconds":30},
            {"name":"standards", "instructions":"Standards specialization marker", "allowed_tools":tools, "allowed_paths":".", "seconds":30},
            {"name":"simplifier", "instructions":"Simplifier specialization marker", "allowed_tools":tools, "allowed_paths":".", "seconds":30}
        ],
        "steps":[
            {"kind":"agent", "id":"code", "agent":"coder", "objective":"Implement code"},
            {"kind":"agent", "id":"rewrite", "agent":"standards", "objective":"Rewrite to repository standards"},
            {"kind":"agent", "id":"simplify", "agent":"simplifier", "objective":"Simplify without changing behavior"}
        ]
    })
}

async fn start_pipeline(actor: &RepositoryActor, input: Value) {
    actor
        .actor
        .send_message(Message::StartWork(Some(
            "Execute the ordered workflow".into(),
        )))
        .unwrap();
    answer(
        actor.request().await.1,
        response(vec![tool("run_workflow", "pipeline", input)]),
    );
}

#[tokio::test]
async fn workflow_runtime_starts_separate_agents_and_passes_prior_findings() {
    let workspace = session::test_support::Workspace::new();
    let actor = RepositoryActor::new(BaseWorker::new(), workspace.path.clone()).await;
    start_pipeline(&actor, workflow_input("find_files")).await;
    let mut previous = Vec::new();
    for marker in [
        "Code specialization marker",
        "Standards specialization marker",
        "Simplifier specialization marker",
    ] {
        let (request, reply) = actor.request().await;
        let messages = serde_json::to_string(&request.messages).unwrap();
        assert!(messages.contains(marker));
        assert!(messages.contains("Shared workflow requirement"));
        for finding in &previous {
            assert!(messages.contains(finding));
        }
        assert!(request.tools.iter().all(
            |tool| matches!(tool, ToolDefinition::Client { name, .. } if name == "find_files")
        ));
        assert!(actor.requests.is_empty());
        answer(reply, response(vec![text(marker)]));
        previous.push(marker);
    }
    let (request, reply) = actor.request().await;
    let result = latest_result(&request);
    assert_eq!(result["status"], "completed", "{result}");
    let ids = result["steps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|step| step["output"]["report"]["worker_id"].as_str().unwrap())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(ids.len(), 3);
    assert_eq!(
        result["steps"][1]["output"]["report"]["findings"],
        "Standards specialization marker"
    );
    completed_root(&actor, reply).await;
    actor.stop().await;
}

#[tokio::test]
async fn workflow_runtime_shares_changes_and_releases_each_writer_before_the_next() {
    let workspace = session::test_support::Workspace::new();
    std::fs::write(workspace.path.join("pipeline.txt"), "original\n").unwrap();
    let actor = RepositoryActor::new(BaseWorker::new(), workspace.path.clone()).await;
    start_pipeline(&actor, workflow_input("knowledge\napply_patch")).await;
    for stage in [
        ("original", "coded"),
        ("coded", "standardized"),
        ("standardized", "simplified"),
    ] {
        let (_, reply) = actor.request().await;
        answer(
            reply,
            response(vec![tool(
                "knowledge",
                "read-current",
                json!({"action":"read", "file_path":"pipeline.txt"}),
            )]),
        );
        let (request, reply) = actor.request().await;
        assert_eq!(
            latest_result(&request)["content"],
            format!("1: {}", stage.0)
        );
        answer(
            reply,
            response(vec![tool(
                "apply_patch",
                "edit-current",
                json!({"patch":format!("*** Begin Patch\n*** Update File: pipeline.txt\n@@\n-{}\n+{}\n*** End Patch", stage.0, stage.1)}),
            )]),
        );
        let (request, reply) = actor.request().await;
        assert_eq!(latest_result(&request)["status"], "ok");
        answer(
            reply,
            response(vec![text(&format!("Finished {}", stage.1))]),
        );
    }
    let (request, reply) = actor.request().await;
    let result = latest_result(&request);
    assert_eq!(result["status"], "completed", "{result}");
    for step in result["steps"].as_array().unwrap() {
        assert_eq!(
            step["output"]["report"]["changed_files"],
            json!(["pipeline.txt"])
        );
    }
    assert_eq!(
        std::fs::read_to_string(workspace.path.join("pipeline.txt")).unwrap(),
        "simplified\n"
    );
    completed_root(&actor, reply).await;
    actor.stop().await;
}

#[tokio::test]
async fn workflow_runtime_stops_after_provider_failure_and_preserves_the_report() {
    let workspace = session::test_support::Workspace::new();
    let actor = RepositoryActor::new(BaseWorker::new(), workspace.path.clone()).await;
    start_pipeline(&actor, workflow_input("find_files")).await;
    let (_, reply) = actor.request().await;
    assert!(
        reply
            .send(Err(Failure::new(
                FailureKind::Authentication,
                "Workflow fixture failure"
            )
            .into()))
            .is_ok()
    );
    let (request, reply) = actor.request().await;
    let result = latest_result(&request);
    assert_eq!(result["status"], "stopped", "{result}");
    assert_eq!(result["steps"][0]["output"]["report"]["status"], "failed");
    assert_eq!(result["steps"][1]["output"]["kind"], "skipped");
    assert!(matches!(
        latest_tool_result(&request),
        ContentBlock::ToolResult {
            is_error: Some(true),
            ..
        }
    ));
    assert!(actor.requests.is_empty());
    completed_root(&actor, reply).await;
    actor.stop().await;
}

#[tokio::test]
async fn workflow_runtime_parent_interrupt_cancels_the_active_agent_and_skips_later_steps() {
    let workspace = session::test_support::Workspace::new();
    let actor = RepositoryActor::new(BaseWorker::new(), workspace.path.clone()).await;
    start_pipeline(&actor, workflow_input("find_files")).await;
    let (_, pending) = actor.request().await;
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
    assert!(pending.is_closed());
    assert!(actor.requests.is_empty());
    actor.stop().await;
}

#[tokio::test]
async fn workflow_runtime_composes_existing_workflows_as_ordered_steps() {
    use worker_registry::workflow::BuiltinAgent;

    let builtins = [
        BuiltinAgent::GatherContext,
        BuiltinAgent::MakeChanges,
        BuiltinAgent::ValidateRust,
    ];
    let steps = builtins
        .iter()
        .map(|builtin| {
            let definition = builtin.definition();
            json!({
                "kind": "agent",
                "id": definition.name,
                "agent": definition.name,
                "objective": format!("Run {} phase", definition.name)
            })
        })
        .collect::<Vec<_>>();
    let workspace = session::test_support::Workspace::new();
    let actor = RepositoryActor::new(BaseWorker::new(), workspace.path.clone()).await;
    start_pipeline(&actor, json!({"steps": steps})).await;
    for builtin in builtins {
        let definition = builtin.definition();
        let (request, reply) = actor.request().await;
        let actual = request
            .tools
            .iter()
            .filter_map(|tool| match tool {
                ToolDefinition::Client { name, .. } => Some(name.as_str()),
                _ => None,
            })
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(actual, definition.allowed_tools.lines().collect());
        answer(
            reply,
            response(vec![text(&format!("Finished {}", definition.name))]),
        );
    }
    let (request, reply) = actor.request().await;
    let result = latest_result(&request);
    assert_eq!(result["status"], "completed", "{result}");
    assert_eq!(result["steps"].as_array().unwrap().len(), 3);
    for (index, builtin) in builtins.iter().enumerate() {
        let name = builtin.definition().name;
        assert_eq!(result["steps"][index]["id"], name);
        assert_eq!(
            result["steps"][index]["output"]["report"]["findings"],
            format!("Finished {name}")
        );
    }
    completed_root(&actor, reply).await;
    actor.stop().await;
}

#[tokio::test]
async fn workflow_runtime_existing_tools_keep_their_worker_report_interface() {
    for builtin in [
        worker_registry::workflow::BuiltinAgent::GatherContext,
        worker_registry::workflow::BuiltinAgent::MakeChanges,
        worker_registry::workflow::BuiltinAgent::ValidateRust,
    ] {
        let definition = builtin.definition();
        let workspace = session::test_support::Workspace::new();
        let actor = RepositoryActor::new(BaseWorker::new(), workspace.path.clone()).await;
        actor
            .actor
            .send_message(Message::StartWork(Some("Run the existing workflow".into())))
            .unwrap();
        answer(
            actor.request().await.1,
            response(vec![tool(
                &definition.name,
                "existing",
                json!({"context":"Existing bounded objective"}),
            )]),
        );
        let (worker, reply) = actor.request().await;
        assert!(
            serde_json::to_string(&worker.messages)
                .unwrap()
                .contains("Existing bounded objective")
        );
        let actual = worker
            .tools
            .iter()
            .filter_map(|tool| match tool {
                ToolDefinition::Client { name, .. } => Some(name.as_str()),
                _ => None,
            })
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(actual, definition.allowed_tools.lines().collect());
        answer(reply, response(vec![text("Existing workflow finished")]));
        let (request, reply) = actor.request().await;
        let result = latest_result(&request);
        assert_eq!(result["status"], "completed");
        assert_eq!(result["findings"], "Existing workflow finished");
        assert!(result["worker_id"].is_string());
        assert!(result.get("steps").is_none());
        completed_root(&actor, reply).await;
        actor.stop().await;
    }
}
