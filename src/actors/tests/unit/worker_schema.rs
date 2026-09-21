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
