use super::*;
use crate::failure::{Failure, FailureKind};

#[test]
fn cargo_selection_schemas_survive_both_provider_mappings() {
    use tools::tool_defs::ToolDefTrait;
    let properties = <tools::cargo_tools::Cargo as ToolDefTrait>::field_properties();
    for name in [
        "operation",
        "target",
        "args",
        "environment",
        "process_id",
        "offsets",
    ] {
        let property = properties[name].clone();
        let expected = match &property {
            tools::tool_defs::ToolProperty::Schema(schema) => schema.clone(),
            _ => panic!("Expected a structured schema"),
        };
        let openai: crate::openai::ToolProperty = property.clone().into();
        let claude: crate::claude::ToolProperty = property.into();
        assert_eq!(serde_json::to_value(&openai).unwrap(), expected);
        assert_eq!(serde_json::to_value(&claude).unwrap(), expected);
        assert_eq!(
            serde_json::to_value(
                serde_json::from_value::<crate::openai::ToolProperty>(expected.clone()).unwrap()
            )
            .unwrap(),
            expected
        );
        assert_eq!(
            serde_json::to_value(
                serde_json::from_value::<crate::claude::ToolProperty>(expected.clone()).unwrap()
            )
            .unwrap(),
            expected
        );
    }
}

#[test]
fn derived_input_schemas_survive_both_provider_mappings() {
    use tools::tool_defs::ToolDefTrait;
    for properties in [
        tools::git::Git::field_properties(),
        tools::worktree::Worktree::field_properties(),
        tools::read_file::ReadFile::field_properties(),
    ] {
        for property in properties.into_values() {
            let expected = property.clone().into_schema();
            let openai: crate::openai::ToolProperty = property.clone().into();
            let claude: crate::claude::ToolProperty = property.into();
            assert_eq!(serde_json::to_value(openai).unwrap(), expected);
            assert_eq!(serde_json::to_value(claude).unwrap(), expected);
        }
    }
}

#[test]
fn native_compaction_replays_the_entire_window_without_altering_opaque_or_retained_items() {
    let items = serde_json::json!([
        {"type": "message", "id": "msg-old", "role": "user", "content": [{"type": "input_text", "text": "keep my requirements"}], "future": [1, 2]},
        {"type": "compaction", "id": "cmp-1", "encrypted_content": "opaque-data", "future": {"a": true}},
        {"type": "message", "id": "msg-recent", "role": "assistant", "phase": "commentary", "status": "completed", "content": [{"type": "output_text", "text": "retained", "annotations": []}]}
    ]);
    let message = llm::Message {
        role: llm::Role::Assistant,
        content: vec![ContentBlock::OpenAICompaction(
            serde_json::from_value(items.clone()).unwrap(),
        )],
    };
    let request: ClientRequest = llm::ClientRequest::new(vec![message.clone()])
        .with_output_limit(2048)
        .try_into()
        .unwrap();
    assert_eq!(serde_json::to_value(request.input).unwrap(), items);
    assert_eq!(request.max_output_tokens, Some(2048));
    assert!(
        crate::claude::ClientRequest::try_from(llm::ClientRequest::new(vec![message])).is_err()
    );
}

#[test]
fn incomplete_response_preserves_the_structured_reason() {
    for (reason, kind) in [
        ("max_output_tokens", FailureKind::Truncation),
        ("context_length_exceeded", FailureKind::ContextOverflow),
        ("content_filter", FailureKind::InvalidInput),
    ] {
        let event: openai::StreamEvent = serde_json::from_value(serde_json::json!({
            "type": "response.incomplete", "sequence_number": 1,
            "response": {"incomplete_details": {"reason": reason}}
        }))
        .unwrap();
        let Some(llm::StreamEvent::Error { error }) = Option::<llm::StreamEvent>::from(event)
        else {
            panic!("incomplete response must fail");
        };
        let failure = Failure::api(&error.error_type, &error.message);
        assert_eq!(failure.kind, kind);
        assert!(!failure.retryable());
    }
}
