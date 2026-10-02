#[test]
fn worker_tool_schema_has_no_request_limit() {
    use tools::tool_defs::ToolDefTrait;

    let properties = crate::tools::start_worker::StartWorker::field_properties();
    assert!(!properties.contains_key("requests"));
    assert!(properties.contains_key("seconds"));
}

#[test]
fn convenience_tools_allow_the_full_worker_deadline() {
    use crate::actor::ActorContext;
    use analysis::contexts::rust_context::RustContext;
    use tools::tool_defs::ToolTrait;

    let expected = std::time::Duration::from_secs(1800);
    assert_eq!(
        <crate::tools::gather_context::GatherContext as ToolTrait<
            RustContext,
            ActorContext<RustContext>,
        >>::execution_budget(&Default::default())
        .unwrap(),
        expected
    );
    assert_eq!(
        <crate::tools::make_changes::MakeChanges as ToolTrait<
            RustContext,
            ActorContext<RustContext>,
        >>::execution_budget(&Default::default())
        .unwrap(),
        expected
    );
    assert_eq!(
        <crate::tools::validate_rust::ValidateRust as ToolTrait<
            RustContext,
            ActorContext<RustContext>,
        >>::execution_budget(&Default::default())
        .unwrap(),
        expected
    );
}

#[test]
fn workflow_permissions_and_deadlines_follow_the_configured_steps() {
    use crate::actor::ActorContext;
    use crate::tools::run_workflow::RunWorkflow;
    use analysis::contexts::rust_context::RustContext;
    use tools::tool_defs::{ToolOpKind, ToolTrait};
    use workflows::WorkflowInput;

    let read: WorkflowInput = serde_json::from_value(serde_json::json!({"steps":[
        {"kind":"agent", "id":"read", "agent":"gather_context", "objective":"Inspect"}
    ]}))
    .unwrap();
    let write: WorkflowInput = serde_json::from_value(serde_json::json!({"steps":[
        {"kind":"agent", "id":"read", "agent":"gather_context", "objective":"Inspect"},
        {"kind":"agent", "id":"write", "agent":"make_changes", "objective":"Implement"},
        {"kind":"agent", "id":"validate", "agent":"validate_rust", "objective":"Check"}
    ]}))
    .unwrap();
    assert_eq!(
        <RunWorkflow as ToolTrait<RustContext, ActorContext<RustContext>>>::effect_from_input(
            &read
        ),
        ToolOpKind::DelegateRead
    );
    assert_eq!(
        <RunWorkflow as ToolTrait<RustContext, ActorContext<RustContext>>>::effect_from_input(
            &write
        ),
        ToolOpKind::DelegateWrite
    );
    assert_eq!(
        <RunWorkflow as ToolTrait<RustContext, ActorContext<RustContext>>>::execution_budget(
            &write
        )
        .unwrap(),
        std::time::Duration::from_secs(5400)
    );
}
