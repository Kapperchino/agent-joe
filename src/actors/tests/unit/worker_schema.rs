#[test]
fn worker_tool_schema_has_no_request_limit() {
    use tools::tool_defs::ToolDefTrait;

    let properties = crate::tools::start_worker::StartWorker::field_properties();
    assert!(!properties.contains_key("requests"));
    assert!(properties.contains_key("seconds"));
}
