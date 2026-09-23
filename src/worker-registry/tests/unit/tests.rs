use super::*;
use report::WorkerOutcome;
use request::WorkerRequestInput;

fn request() -> WorkerRequest {
    WorkerRequest::new(
        WorkerRequestInput {
            objective: "Investigate a bounded question".into(),
            allowed_tools: "read_file".into(),
            allowed_paths: ".".into(),
            completion_criteria: "Report evidence".into(),
            ..Default::default()
        },
        |_| Some(tools::tool_defs::ToolOpKind::Read),
    )
    .unwrap()
}

#[test]
fn cancellation_and_late_updates_preserve_reports_until_collection() {
    let registry = WorkerRegistry::default();
    let scope = CancellationToken::new();
    let worker = registry
        .register("parent", scope.clone(), request())
        .unwrap();
    assert_eq!(
        registry.status("parent", &worker.id).unwrap().status,
        WorkerStatus::Registered
    );
    assert_eq!(registry.pending("parent").len(), 1);
    registry.cancel("parent", &worker.id).unwrap();
    registry.running("parent", &worker.id);
    assert!(scope.is_cancelled());
    assert_eq!(registry.list("parent")[0].status, WorkerStatus::Cancelling);
    assert!(registry.cleanup("parent", &worker.id).is_err());
    registry.complete(
        "parent",
        worker.report(
            WorkerOutcome::Cancelled,
            std::time::Duration::ZERO,
            Default::default(),
        ),
    );
    registry.cancel("parent", &worker.id).unwrap();
    registry.running("parent", &worker.id);
    registry.complete(
        "parent",
        worker.report(
            WorkerOutcome::Failed("late failure".into()),
            std::time::Duration::ZERO,
            Default::default(),
        ),
    );
    assert_eq!(registry.list("parent")[0].status, WorkerStatus::Cancelled);
    assert_eq!(registry.pending("parent").len(), 1);
    let collected = registry.status("parent", &worker.id).unwrap();
    assert_eq!(collected.status, WorkerStatus::Cancelled);
    assert_eq!(
        collected.report.unwrap().findings,
        "Worker cancelled after cleanup"
    );
    assert!(registry.pending("parent").is_empty());
    registry.running("parent", &worker.id);
    registry.complete(
        "parent",
        worker.report(
            WorkerOutcome::Completed("late success".into()),
            std::time::Duration::ZERO,
            Default::default(),
        ),
    );
    assert!(registry.pending("parent").is_empty());
    assert_eq!(
        registry.cleanup("parent", &worker.id).unwrap().status,
        WorkerStatus::Cancelled
    );
}

#[test]
fn recovery_requires_a_matching_terminal_report_and_preserves_completed_evidence() {
    let registry = WorkerRegistry::default();
    let worker = registry
        .register("parent", CancellationToken::new(), request())
        .unwrap();
    let report = worker.report(
        WorkerOutcome::Completed("Saved evidence".into()),
        std::time::Duration::ZERO,
        Default::default(),
    );
    let completed = WorkerView {
        worker_id: worker.id.clone(),
        request: worker.request.clone(),
        status: WorkerStatus::Completed,
        report: Some(report.clone()),
    };
    for mut saved in [
        WorkerView {
            report: None,
            ..completed.clone()
        },
        WorkerView {
            status: WorkerStatus::Running,
            ..completed.clone()
        },
        WorkerView {
            report: Some(WorkerReport {
                worker_id: "another-worker".into(),
                ..report
            }),
            ..completed.clone()
        },
    ] {
        saved.recover();
        assert_eq!(saved.status, WorkerStatus::Interrupted);
        assert_eq!(saved.report.as_ref().unwrap().worker_id, worker.id);
        assert!(saved.report.as_ref().unwrap().unresolved_issues[0].contains("uncertain"));
        saved.recover();
        assert_eq!(saved.status, WorkerStatus::Interrupted);
    }
    let encoded = serde_json::to_value(&completed).unwrap();
    let mut restored: WorkerView = serde_json::from_value(encoded.clone()).unwrap();
    restored.recover();
    assert_eq!(serde_json::to_value(&restored).unwrap(), encoded);
    registry.restore("resumed", BTreeMap::from([(worker.id.clone(), restored)]));
    assert_eq!(
        registry.list("resumed")[0]
            .report
            .as_ref()
            .unwrap()
            .findings,
        "Saved evidence"
    );
    assert!(registry.pending("resumed").is_empty());
}

#[tokio::test]
async fn registry_retains_immediate_completions_and_enforces_owner_and_worker_counts() {
    let registry = WorkerRegistry::default();
    let workers = (0..4)
        .map(|_| {
            registry
                .register("parent", CancellationToken::new(), request())
                .unwrap()
        })
        .collect::<Vec<_>>();
    assert!(
        registry
            .register("parent", CancellationToken::new(), request())
            .is_err()
    );
    assert!(registry.status("other", &workers[0].id).is_err());
    assert!(registry.cancel("other", &workers[0].id).is_err());
    assert!(registry.cleanup("parent", &workers[0].id).is_err());
    for worker in &workers {
        registry.complete(
            "parent",
            worker.report(
                WorkerOutcome::Completed("immediate result".into()),
                std::time::Duration::ZERO,
                Default::default(),
            ),
        );
        let view = registry.wait("parent", &worker.id, 0).await.unwrap();
        assert_eq!(view.report.unwrap().findings, "immediate result");
        registry.cleanup("parent", &worker.id).unwrap();
    }
    assert!(registry.pending("parent").is_empty());
    for _ in 4..32 {
        let worker = registry
            .register("parent", CancellationToken::new(), request())
            .unwrap();
        registry.complete(
            "parent",
            worker.report(
                WorkerOutcome::Completed(String::new()),
                std::time::Duration::ZERO,
                Default::default(),
            ),
        );
        registry.cleanup("parent", &worker.id).unwrap();
    }
    assert!(
        registry
            .register("parent", CancellationToken::new(), request())
            .is_err()
    );
}

#[test]
fn reports_combine_observed_changes_with_supplied_evidence() {
    use tools::tool_defs::{ToolId, ToolInvocation, ToolOpKind, ToolResult};
    let registry = WorkerRegistry::default();
    let worker = registry
        .register("parent", CancellationToken::new(), request())
        .unwrap();
    worker.record(ToolOpKind::Write, &ToolResult {
        id: ToolId { call_id: None, id: "edit".to_owned().try_into().unwrap() },
        invocation: ToolInvocation {
            name: "apply_patch".to_owned().try_into().unwrap(),
            input: serde_json::from_value(serde_json::json!({"patch": "*** Begin Patch\n*** Add File: result.txt\n+evidence\n*** End Patch"})).unwrap(),
            display: String::new(),
        },
        outcome: Ok("applied".into()),
    });
    let report = worker.report(
        WorkerOutcome::Completed("Observed evidence".into()),
        std::time::Duration::from_millis(75),
        report::StoredWorkerEvidence {
            artifacts: vec![
                utils::artifacts::ArtifactReference::new("artifact-1".into(), 4096).unwrap(),
            ],
            unresolved_issues: vec!["One process record could not be retrieved".into()],
            ..Default::default()
        },
    );
    assert_eq!(report.changed_files, ["result.txt"]);
    assert_eq!(report.artifacts[0].id, "artifact-1");
    assert_eq!(
        report.unresolved_issues,
        ["One process record could not be retrieved"]
    );
    assert_eq!(report.duration_ms, 75);
    assert_eq!(report.status, WorkerStatus::Completed);
}

#[test]
fn reports_preserve_budget_failure_and_truncate_at_utf8_boundaries() {
    let registry = WorkerRegistry::default();
    let worker = registry
        .register("parent", CancellationToken::new(), request())
        .unwrap();
    for _ in 0..128 {
        worker.budget.tool_call().unwrap();
    }
    assert!(worker.budget.tool_call().is_err());
    let findings = "界".repeat(3000);
    let report = worker.report(
        WorkerOutcome::Completed(findings.clone()),
        std::time::Duration::ZERO,
        Default::default(),
    );
    assert_eq!(report.status, WorkerStatus::BudgetExhausted);
    assert!(report.findings.len() <= 8192);
    assert!(findings.starts_with(&report.findings));
    assert!(
        report
            .unresolved_issues
            .iter()
            .any(|issue| issue.contains("truncated"))
    );
}
