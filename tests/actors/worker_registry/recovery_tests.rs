use super::*;

#[test]
fn recovery_exposes_uncertain_work_and_preserves_limits_across_forks() {
    let request = WorkerRequest::new(
        request::WorkerRequestInput {
            objective: "Investigate".into(),
            allowed_tools: "read_file".into(),
            allowed_paths: ".".into(),
            completion_criteria: "Report evidence".into(),
            tokens: Some(500_000),
            ..Default::default()
        },
        |_| Some(tools::tool_defs::ToolEffect::Read),
    )
    .unwrap();
    let mut view = WorkerView {
        worker_id: "saved-worker".into(),
        request: request.clone(),
        status: WorkerStatus::Running,
        report: None,
    };
    view.recover();
    assert_eq!(view.status, WorkerStatus::Interrupted);
    assert!(view.report.as_ref().unwrap().unresolved_issues[0].contains("uncertain"));
    let registry = WorkerRegistry::default();
    let saved = (0..4)
        .map(|index| {
            let id = format!("saved-{index}");
            (
                id.clone(),
                WorkerView {
                    worker_id: id,
                    ..view.clone()
                },
            )
        })
        .collect::<BTreeMap<_, _>>();
    registry.restore("source", saved.clone());
    registry.restore("fork", saved);
    assert_eq!(registry.list("source").len(), 4);
    assert_eq!(registry.list("fork").len(), 4);
    assert!(
        registry
            .register("source", ExecutionScope::default(), request.clone())
            .is_err()
    );
    assert!(
        registry
            .register("fork", ExecutionScope::default(), request)
            .is_err()
    );
    registry.cleanup("fork", "saved-0").unwrap();
    assert!(registry.status("source", "saved-0").is_ok());
}
