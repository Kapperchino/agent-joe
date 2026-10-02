use super::*;
use serde_json::{Value, json};
use std::cell::RefCell;

fn available(name: &str) -> Option<ToolOpKind> {
    match name {
        "apply_patch" | "undo_changes" => Some(ToolOpKind::Write),
        "cargo" => Some(ToolOpKind::Validate),
        "start_worker" => Some(ToolOpKind::DelegateRead),
        "find_files" | "list_directory" | "knowledge" | "grep" | "git" | "review_changes" => {
            Some(ToolOpKind::Read)
        }
        _ => None,
    }
}

fn input(value: Value) -> WorkflowInput {
    serde_json::from_value(value).unwrap()
}

fn report(request: &WorkerRequest, status: WorkerStatus) -> WorkerReport {
    WorkerReport {
        worker_id: request.objective().into(),
        status,
        findings: format!("Finished {}", request.objective()),
        changed_files: vec!["src/lib.rs".into()],
        possibly_changed_files: Vec::new(),
        validation: Vec::new(),
        edits: Vec::new(),
        processes: Vec::new(),
        unresolved_issues: Vec::new(),
        artifacts: Vec::new(),
        budget: Default::default(),
        duration_ms: 0,
        completion_criteria: request.completion_criteria().into(),
    }
}

#[tokio::test]
async fn workflow_runs_three_distinct_agents_in_order_with_handoffs() {
    let workflow = Workflow::new(input(json!({
        "context": "Implement the requested behavior",
        "agents": [
            {"name":"coder", "instructions":"Write the code", "allowed_tools":"apply_patch\nknowledge", "allowed_paths":"src", "seconds":30},
            {"name":"standards", "instructions":"Rewrite to scoped repository standards", "allowed_tools":"apply_patch\nknowledge", "allowed_paths":"src", "seconds":40},
            {"name":"simplifier", "instructions":"Simplify without changing behavior", "allowed_tools":"apply_patch\nknowledge", "allowed_paths":"src", "seconds":50}
        ],
        "steps": [
            {"kind":"context", "id":"requirements", "content":"Preserve the public API"},
            {"kind":"agent", "id":"code", "agent":"coder", "objective":"code", "context":"selected source", "completion_criteria":"Behavior implemented"},
            {"kind":"agent", "id":"rewrite", "agent":"standards", "objective":"rewrite"},
            {"kind":"agent", "id":"simplify", "agent":"simplifier", "objective":"simplify"}
        ]
    })), available).unwrap();
    assert_eq!(workflow.effect(), ToolOpKind::DelegateWrite);
    assert_eq!(workflow.execution_budget(), Duration::from_secs(120));
    let requests = RefCell::new(Vec::new());
    let result = workflow
        .run(|request| {
            let result = report(&request, WorkerStatus::Completed);
            requests.borrow_mut().push(request);
            std::future::ready(Ok(result))
        })
        .await;
    assert_eq!(result.status, WorkflowStatus::Completed);
    let encoded = serde_json::to_value(&result).unwrap();
    let decoded: WorkflowReport = serde_json::from_value(encoded).unwrap();
    assert_eq!(decoded.status, WorkflowStatus::Completed);
    let requests = requests.into_inner();
    assert_eq!(
        requests
            .iter()
            .map(WorkerRequest::objective)
            .collect::<Vec<_>>(),
        ["code", "rewrite", "simplify"]
    );
    assert!(
        requests[1]
            .prompt(&["Inherited user requirement".into()])
            .unwrap()
            .contains("Rewrite to scoped repository standards")
    );
    assert_eq!(
        requests[0].allowed_paths(),
        [std::path::PathBuf::from("src")]
    );
    assert_eq!(requests[0].completion_criteria(), "Behavior implemented");
    let handoff: Value = serde_json::from_str(requests[2].context()).unwrap();
    assert_eq!(
        handoff["workflow_context"],
        "Implement the requested behavior"
    );
    assert_eq!(
        handoff["previous_steps"][0]["output"]["content"],
        "Preserve the public API"
    );
    assert_eq!(
        handoff["previous_steps"][1]["output"]["findings"],
        "Finished code"
    );
    assert_eq!(
        handoff["previous_steps"][2]["output"]["changed_files"],
        json!(["src/lib.rs"])
    );
    let first: Value = serde_json::from_str(requests[0].context()).unwrap();
    assert_eq!(first["step_context"], "selected source");
}

#[tokio::test]
async fn workflow_stops_on_every_noncompleted_worker_status() {
    for status in [
        WorkerStatus::Failed,
        WorkerStatus::Cancelled,
        WorkerStatus::TimedOut,
        WorkerStatus::BudgetExhausted,
        WorkerStatus::Interrupted,
    ] {
        let workflow = Workflow::new(
            input(json!({"steps":[
                {"kind":"agent", "id":"first", "agent":"gather_context", "objective":"first"},
                {"kind":"agent", "id":"second", "agent":"make_changes", "objective":"second"},
                {"kind":"context", "id":"later", "content":"Must not run"}
            ]})),
            available,
        )
        .unwrap();
        let calls = RefCell::new(Vec::new());
        let result = workflow
            .run(|request| {
                calls.borrow_mut().push(request.objective().to_owned());
                std::future::ready(Ok(report(&request, status)))
            })
            .await;
        assert_eq!(result.status, WorkflowStatus::Stopped);
        assert_eq!(calls.into_inner(), ["first"]);
        assert!(matches!(result.steps[1].output, StepOutput::Skipped));
        assert!(matches!(result.steps[2].output, StepOutput::Skipped));
    }
}

#[tokio::test]
async fn workflow_retains_prior_reports_when_a_later_launch_fails() {
    let workflow = Workflow::new(
        input(json!({"steps":[
            {"kind":"agent", "id":"first", "agent":"gather_context", "objective":"first"},
            {"kind":"agent", "id":"second", "agent":"make_changes", "objective":"second"},
            {"kind":"agent", "id":"third", "agent":"validate_rust", "objective":"third"}
        ]})),
        available,
    )
    .unwrap();
    let result = workflow
        .run(|request| {
            std::future::ready(match request.objective() {
                "first" => Ok(report(&request, WorkerStatus::Completed)),
                _ => Err(anyhow::anyhow!("A writer already owns the workspace")),
            })
        })
        .await;
    assert_eq!(result.status, WorkflowStatus::Stopped);
    assert!(matches!(result.steps[0].output, StepOutput::Agent { .. }));
    assert!(
        matches!(&result.steps[1].output, StepOutput::Failed { message } if message.contains("writer"))
    );
    assert!(matches!(result.steps[2].output, StepOutput::Skipped));
}

#[tokio::test]
async fn existing_workflows_use_the_same_runner_and_keep_their_allowances() {
    for agent in [
        BuiltinAgent::GatherContext,
        BuiltinAgent::MakeChanges,
        BuiltinAgent::ValidateRust,
    ] {
        let definition = agent.definition();
        let workflow =
            Workflow::single(agent, "Existing workflow request".into(), available).unwrap();
        assert_eq!(workflow.execution_budget(), Duration::from_secs(1800));
        let result = workflow
            .run(|request| {
                assert_eq!(request.allowed_tools().join("\n"), definition.allowed_tools);
                assert_eq!(request.allowed_paths(), [std::path::PathBuf::from(".")]);
                assert_eq!(request.objective(), "Existing workflow request");
                std::future::ready(Ok(report(&request, WorkerStatus::Completed)))
            })
            .await;
        assert_eq!(result.status, WorkflowStatus::Completed);
        assert_eq!(result.steps.len(), 1);
        assert_eq!(result.steps[0].id, definition.name);
    }
}

#[tokio::test]
async fn context_steps_do_not_launch_workers() {
    let workflow = Workflow::new(
        input(json!({"steps":[{"kind":"context", "id":"only", "content":"Reference material"}]})),
        available,
    )
    .unwrap();
    assert_eq!(workflow.effect(), ToolOpKind::DelegateRead);
    assert_eq!(workflow.execution_budget(), Duration::ZERO);
    let result = workflow
        .run(|_| std::future::ready(Err(anyhow::anyhow!("Must not launch"))))
        .await;
    assert_eq!(result.status, WorkflowStatus::Completed);
}

#[test]
fn workflow_rejects_invalid_configuration_before_execution() {
    let valid = json!({"steps":[{"kind":"agent", "id":"read", "agent":"gather_context", "objective":"Inspect"}]});
    let invalid = [
        json!({"steps":[]}),
        json!({"steps":[{"kind":"context", "id":"", "content":"Context"}]}),
        json!({"steps":[{"kind":"context", "id":"empty", "content":" "}]}),
        json!({"steps":[{"kind":"agent", "id":"read", "agent":"missing", "objective":"Inspect"}]}),
        json!({"steps":[{"kind":"agent", "id":"read", "agent":"gather_context", "objective":" "}]}),
        json!({"steps":[{"kind":"context", "id":"duplicate", "content":"one"},{"kind":"context", "id":"duplicate", "content":"two"}]}),
        json!({"agents":[{"name":"gather_context", "allowed_tools":"find_files", "allowed_paths":"."}], "steps":valid["steps"]}),
        json!({"agents":[{"name":"custom", "allowed_tools":"start_worker", "allowed_paths":"."}], "steps":[{"kind":"agent", "id":"run", "agent":"custom", "objective":"Do work"}]}),
        json!({"agents":[{"name":"custom", "allowed_tools":"missing", "allowed_paths":"."}], "steps":[{"kind":"agent", "id":"run", "agent":"custom", "objective":"Do work"}]}),
        json!({"agents":[{"name":"custom", "allowed_tools":"find_files", "allowed_paths":"../outside"}], "steps":[{"kind":"agent", "id":"run", "agent":"custom", "objective":"Do work"}]}),
        json!({"agents":[{"name":"custom", "allowed_tools":"find_files", "allowed_paths":".", "seconds":0}], "steps":[{"kind":"agent", "id":"run", "agent":"custom", "objective":"Do work"}]}),
        json!({"context":"x".repeat(65536), "steps":valid["steps"]}),
    ];
    for value in invalid {
        assert!(
            Workflow::new(input(value.clone()), available).is_err(),
            "{value}"
        );
    }
    let mut too_many = input(valid);
    too_many.steps = (0..17)
        .map(|id| StepInput::Context {
            id: id.to_string(),
            content: "Context".into(),
        })
        .collect();
    assert!(Workflow::new(too_many, available).is_err());
}

#[tokio::test]
async fn oversized_handoffs_stop_without_discarding_previous_reports() {
    let workflow = Workflow::new(
        input(json!({"steps":[
            {"kind":"agent", "id":"first", "agent":"gather_context", "objective":"first"},
            {"kind":"agent", "id":"second", "agent":"gather_context", "objective":"second"}
        ]})),
        available,
    )
    .unwrap();
    let calls = RefCell::new(0);
    let result = workflow
        .run(|request| {
            *calls.borrow_mut() += 1;
            let mut report = report(&request, WorkerStatus::Completed);
            report.findings = "多".repeat(24000);
            std::future::ready(Ok(report))
        })
        .await;
    assert_eq!(calls.into_inner(), 1);
    assert_eq!(result.status, WorkflowStatus::Stopped);
    assert!(
        matches!(&result.steps[1].output, StepOutput::Failed { message } if message.contains("64 KiB"))
    );
    assert!(
        matches!(&result.steps[0].output, StepOutput::Agent { report } if report.findings.len() == 72000)
    );
}
