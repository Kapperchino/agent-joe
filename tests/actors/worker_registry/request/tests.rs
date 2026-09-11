use super::*;

#[test]
fn followups_recheck_handoff_bounds_and_preserve_the_original_constraints() {
    let request = WorkerRequest::new(
        WorkerRequestInput {
            objective: "Inspect selected files".into(),
            constraints: "Preserve public APIs".into(),
            allowed_tools: "read_file".into(),
            allowed_paths: "src".into(),
            completion_criteria: "Report evidence".into(),
            ..Default::default()
        },
        |_| Some(ToolEffect::Read),
    )
    .unwrap();
    let followup = request
        .follow_up("Confirm finding".into(), "Selected report".into(), |_| {
            Some(ToolEffect::Read)
        })
        .unwrap();
    assert_eq!(followup.constraints, request.constraints);
    assert_eq!(followup.allowed_tools, request.allowed_tools);
    assert_eq!(followup.allowed_paths, request.allowed_paths);
    assert_eq!(followup.completion_criteria, request.completion_criteria);
    assert_eq!(followup.budget.tokens(), request.budget.tokens());
    assert_eq!(followup.budget.seconds(), request.budget.seconds());
    assert_eq!(followup.budget.requests(), request.budget.requests());
    assert!(
        request
            .follow_up("Confirm finding".into(), "x".repeat(64 * 1024), |_| Some(
                ToolEffect::Read
            ))
            .is_err()
    );
    assert!(
        request
            .follow_up(String::new(), String::new(), |_| Some(ToolEffect::Read))
            .is_err()
    );
    assert!(
        request
            .follow_up("Confirm finding".into(), String::new(), |_| None)
            .is_err()
    );
}

#[test]
fn deserialized_budgets_cannot_bypass_constructor_limits() {
    for input in [
        serde_json::json!({"tokens": 1023, "seconds": 1, "requests": 1}),
        serde_json::json!({"tokens": 500001, "seconds": 1, "requests": 1}),
        serde_json::json!({"tokens": 1024, "seconds": 0, "requests": 1}),
        serde_json::json!({"tokens": 1024, "seconds": 301, "requests": 1}),
        serde_json::json!({"tokens": 1024, "seconds": 1, "requests": 0}),
        serde_json::json!({"tokens": 1024, "seconds": 1, "requests": 33}),
    ] {
        assert!(serde_json::from_value::<BudgetLimits>(input).is_err());
    }
    let limits = BudgetLimits::new(500_000, 300, 32).unwrap();
    let restored: BudgetLimits =
        serde_json::from_value(serde_json::to_value(limits).unwrap()).unwrap();
    assert_eq!(restored.tokens(), limits.tokens());
    assert_eq!(restored.seconds(), limits.seconds());
    assert_eq!(restored.requests(), limits.requests());
}
