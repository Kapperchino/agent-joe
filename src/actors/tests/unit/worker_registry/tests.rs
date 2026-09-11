use super::*;
use launch::WorkerOutcome;
use request::WorkerRequestInput;

fn request(tokens: usize) -> WorkerRequest {
    WorkerRequest::new(
        WorkerRequestInput {
            objective: "Investigate a bounded question".into(),
            allowed_tools: "read_file".into(),
            allowed_paths: ".".into(),
            completion_criteria: "Report evidence".into(),
            tokens: Some(tokens),
            ..Default::default()
        },
        |_| Some(tools::tool_defs::ToolEffect::Read),
    )
    .unwrap()
}

#[test]
fn cancellation_and_late_updates_preserve_reports_until_collection() {
    let registry = WorkerRegistry::default();
    let scope = ExecutionScope::default();
    let worker = registry
        .register("parent", scope.clone(), request(1024))
        .unwrap();
    assert_eq!(
        registry.status("parent", &worker.id).unwrap().status,
        WorkerStatus::Registered
    );
    assert_eq!(registry.pending("parent").len(), 1);
    registry.cancel("parent", &worker.id).unwrap();
    registry.running("parent", &worker.id);
    assert!(scope.cancel.is_cancelled());
    assert_eq!(registry.list("parent")[0].status, WorkerStatus::Cancelling);
    assert!(registry.cleanup("parent", &worker.id).is_err());
    registry.complete(
        "parent",
        worker.report(WorkerOutcome::Cancelled, std::time::Instant::now()),
    );
    registry.cancel("parent", &worker.id).unwrap();
    registry.running("parent", &worker.id);
    registry.complete(
        "parent",
        worker.report(
            WorkerOutcome::Failed("late failure".into()),
            std::time::Instant::now(),
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
            std::time::Instant::now(),
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
        .register("parent", ExecutionScope::default(), request(1024))
        .unwrap();
    let report = worker.report(
        WorkerOutcome::Completed("Saved evidence".into()),
        std::time::Instant::now(),
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
async fn registry_retains_immediate_completions_and_enforces_owner_count_and_total_budgets() {
    let registry = WorkerRegistry::default();
    let workers = (0..4)
        .map(|_| {
            registry
                .register("parent", ExecutionScope::default(), request(1024))
                .unwrap()
        })
        .collect::<Vec<_>>();
    assert!(
        registry
            .register("parent", ExecutionScope::default(), request(1024))
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
                std::time::Instant::now(),
            ),
        );
        let view = registry.wait("parent", &worker.id, 0).await.unwrap();
        assert_eq!(view.report.unwrap().findings, "immediate result");
        registry.cleanup("parent", &worker.id).unwrap();
    }
    assert!(registry.pending("parent").is_empty());
    for _ in 4..32 {
        let worker = registry
            .register("parent", ExecutionScope::default(), request(1024))
            .unwrap();
        registry.complete(
            "parent",
            worker.report(
                WorkerOutcome::Completed(String::new()),
                std::time::Instant::now(),
            ),
        );
        registry.cleanup("parent", &worker.id).unwrap();
    }
    assert!(
        registry
            .register("parent", ExecutionScope::default(), request(1024))
            .is_err()
    );
    for _ in 0..4 {
        let worker = registry
            .register("budget-parent", ExecutionScope::default(), request(500_000))
            .unwrap();
        registry.complete(
            "budget-parent",
            worker.report(
                WorkerOutcome::Completed(String::new()),
                std::time::Instant::now(),
            ),
        );
    }
    assert!(
        registry
            .register("budget-parent", ExecutionScope::default(), request(1024))
            .is_err()
    );
}
