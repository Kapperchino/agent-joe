use super::*;

#[test]
fn qualified_worker_tools_use_registered_names() {
    for allowed_tools in [
        "find_files\nread_file",
        " functions.find_files \n\n functions.read_file ",
    ] {
        let request = WorkerRequest::new(
            WorkerRequestInput {
                objective: "Inspect selected files".into(),
                allowed_tools: allowed_tools.into(),
                allowed_paths: "src".into(),
                completion_criteria: "Report evidence".into(),
                ..Default::default()
            },
            |name| match name {
                "find_files" | "read_file" => Some(ToolEffect::Read),
                _ => None,
            },
        )
        .unwrap();
        assert_eq!(request.allowed_tools, ["find_files", "read_file"]);
        assert_eq!(request.role, WorkerRole::Read);
        assert!(request.allows_tool("find_files"));
        assert!(request.allows_tool("read_file"));
        assert!(!request.allows_tool("apply_patch"));
    }
}

#[test]
fn qualified_worker_tools_cannot_bypass_availability_or_delegation_checks() {
    for name in [
        "missing",
        "functions.missing",
        "functions.",
        "other.read_file",
        "functions.functions.read_file",
        "start_worker",
        "functions.start_worker",
        "functions.make_changes",
    ] {
        let result = WorkerRequest::new(
            WorkerRequestInput {
                objective: "Inspect selected files".into(),
                allowed_tools: name.into(),
                allowed_paths: "src".into(),
                completion_criteria: "Report evidence".into(),
                ..Default::default()
            },
            |name| match name {
                "read_file" => Some(ToolEffect::Read),
                "start_worker" => Some(ToolEffect::DelegateRead),
                "make_changes" => Some(ToolEffect::DelegateWrite),
                _ => None,
            },
        );
        let error = result.unwrap_err().to_string();
        match name {
            "start_worker" | "functions.start_worker" | "functions.make_changes" => {
                assert!(
                    error.contains("delegates; maximum delegation depth is one"),
                    "{error}"
                );
            }
            _ => {
                assert!(error.ends_with("is unavailable"), "{error}");
                assert!(!error.contains("delegation"), "{error}");
            }
        }
    }
}

#[test]
fn followups_recheck_handoff_bounds_and_preserve_the_original_constraints() {
    let request = WorkerRequest::new(
        WorkerRequestInput {
            objective: "Inspect selected files".into(),
            constraints: "Preserve public APIs".into(),
            allowed_tools: "functions.read_file".into(),
            allowed_paths: "src".into(),
            completion_criteria: "Report evidence".into(),
            ..Default::default()
        },
        |name| (name == "read_file").then_some(ToolEffect::Read),
    )
    .unwrap();
    let followup = request
        .follow_up("Confirm finding".into(), "Selected report".into(), |name| {
            (name == "read_file").then_some(ToolEffect::Read)
        })
        .unwrap();
    assert_eq!(followup.constraints, request.constraints);
    assert_eq!(followup.allowed_tools, request.allowed_tools);
    assert_eq!(followup.allowed_paths, request.allowed_paths);
    assert_eq!(followup.completion_criteria, request.completion_criteria);
    assert_eq!(followup.budget.seconds(), request.budget.seconds());
    assert_eq!(
        serde_json::to_value(followup.budget).unwrap(),
        serde_json::json!({"seconds": 180})
    );
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
        serde_json::json!({"seconds": 0}),
        serde_json::json!({"seconds": 301}),
    ] {
        assert!(serde_json::from_value::<BudgetLimits>(input).is_err());
    }
    let limits = BudgetLimits::new(300).unwrap();
    let restored: BudgetLimits =
        serde_json::from_value(serde_json::to_value(limits).unwrap()).unwrap();
    assert_eq!(restored.seconds(), limits.seconds());
}

#[test]
fn legacy_budgets_can_be_restored_without_retaining_token_or_request_limits() {
    let limits: BudgetLimits = serde_json::from_value(serde_json::json!({
        "tokens": 500_000,
        "seconds": 300,
        "requests": 32
    }))
    .unwrap();
    assert_eq!(limits.seconds(), 300);
    assert_eq!(
        serde_json::to_value(limits).unwrap(),
        serde_json::json!({"seconds": 300})
    );
}

#[test]
fn worker_tool_schema_has_no_request_limit() {
    use tools::tool_defs::ToolDefTrait;

    let properties = crate::tools::start_worker::StartWorker::field_properties();
    assert!(!properties.contains_key("requests"));
    assert!(properties.contains_key("seconds"));
}

#[test]
fn legacy_request_limits_are_discarded_from_inputs_and_followups() {
    for requests in [1, 16, 32] {
        let input: WorkerRequestInput = serde_json::from_value(serde_json::json!({
            "objective": "Inspect selected files",
            "constraints": "Preserve public APIs",
            "allowed_tools": "read_file",
            "allowed_paths": "src",
            "context": "Selected context",
            "completion_criteria": "Report evidence",
            "seconds": 30,
            "requests": requests
        }))
        .unwrap();
        assert!(
            serde_json::to_value(&input)
                .unwrap()
                .get("requests")
                .is_none()
        );
        let request = WorkerRequest::new(input, |_| Some(ToolEffect::Read)).unwrap();
        let mut legacy = serde_json::to_value(&request).unwrap();
        legacy["budget"]["requests"] = serde_json::json!(requests);
        let restored: WorkerRequest = serde_json::from_value(legacy).unwrap();
        let followup = restored
            .follow_up("Confirm finding".into(), "Selected report".into(), |_| {
                Some(ToolEffect::Read)
            })
            .unwrap();
        for request in [request, restored, followup] {
            assert_eq!(
                serde_json::to_value(&request).unwrap()["budget"],
                serde_json::json!({"seconds": 30})
            );
            assert!(!request.prompt(&[]).unwrap().contains("\"requests\""));
        }
    }
}
