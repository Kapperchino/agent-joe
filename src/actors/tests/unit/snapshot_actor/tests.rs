use super::*;
use clients::llm::{Message, StreamEvent, StreamProvider};
use conversation::context::estimated_tokens;
use conversation::frozen_context::QUESTION_INSTRUCTIONS;
use futures::{future::BoxFuture, stream::BoxStream};
use std::sync::Arc;

struct NoRequests;

impl StreamProvider for NoRequests {
    fn chat_stream(
        &self,
        _: ClientRequest,
    ) -> BoxFuture<'static, anyhow::Result<BoxStream<'static, anyhow::Result<StreamEvent>>>> {
        Box::pin(async { Err(anyhow::anyhow!("No provider request expected")) })
    }
}

fn client() -> LLmClient {
    LLmClient::Injected(Arc::new(NoRequests))
}

fn context() -> ClientRequest {
    ClientRequest::new(vec![Message::new("All captured context".into())])
        .with_system("Frozen operating instructions".into())
}

#[test]
fn snapshot_questions_use_the_allowance_reserved_at_capture() {
    for ceiling in [4096, 16_000, 64_000] {
        let snapshot = Snapshot::new(
            context(),
            &client(),
            ContextLimits::new(ceiling, 1024).unwrap(),
            Duration::from_secs(1),
        )
        .unwrap();
        let allowance = snapshot.max_question_bytes();
        let request = snapshot.question_request("x".repeat(allowance)).unwrap();
        assert!(estimated_tokens(&request).unwrap() <= snapshot.limits.input());
        assert!(
            snapshot
                .question_request("x".repeat(allowance + 1))
                .is_err()
        );
        assert_eq!(snapshot.context.request().messages.len(), 1);
    }
}

#[test]
fn snapshot_preserves_the_parent_prefix_without_json_wrapping() {
    let limits = ContextLimits::new(272_000, 4096).unwrap();
    let request = ClientRequest::new(vec![Message::new(
        r#"{"path":"src\\lib.rs","text":"println!(\"hello\");\n"}"#.repeat(7800),
    )])
    .with_system("Parent instructions".into())
    .with_prompt_cache_key(Some("parent-cache".into()))
    .with_thinking();
    assert!(estimated_tokens(&request).unwrap() < limits.trigger());
    let parent = request.clone();
    let snapshot =
        Snapshot::for_compaction(request, &client(), limits, Duration::from_secs(1)).unwrap();
    let request = snapshot
        .question_request("What does the source contain?".into())
        .unwrap();
    assert!(estimated_tokens(&request).unwrap() <= limits.input());
    assert_eq!(
        serde_json::to_value(&request.messages[..parent.messages.len()]).unwrap(),
        serde_json::to_value(&parent.messages).unwrap()
    );
    assert_eq!(request.system, parent.system);
    assert_eq!(request.prompt_cache_key, parent.prompt_cache_key);
    assert_eq!(request.thinking, parent.thinking);
    assert_eq!(
        request.messages[parent.messages.len()].text(),
        QUESTION_INSTRUCTIONS
    );
    assert_eq!(
        request.messages.last().unwrap().text(),
        "What does the source contain?"
    );
    assert_eq!(request.max_output_tokens, Some(4096));
}

#[test]
fn oversized_context_is_rejected_without_truncation() {
    let request = ClientRequest::new(vec![Message::new("full context ".repeat(8192))]);
    let result = Snapshot::new(
        request,
        &client(),
        ContextLimits::new(4096, 1024).unwrap(),
        Duration::from_secs(1),
    );
    assert!(
        result
            .err()
            .unwrap()
            .to_string()
            .contains("not truncated or compacted")
    );
}

#[test]
fn native_compaction_and_reasoning_keep_the_same_provider_prefix() {
    let mut parent = context().with_prompt_cache_key(Some("native-parent".into()));
    parent.messages.push(Message {
        role: clients::llm::Role::Assistant,
        content: vec![
            clients::llm::ContentBlock::OpenAICompaction(
                serde_json::from_value(serde_json::json!([
                    {"type": "compaction", "encrypted_content": "opaque-memory", "future": [1, 2]}
                ]))
                .unwrap(),
            ),
            clients::llm::ContentBlock::OpenAIReasoning(
                serde_json::from_value(serde_json::json!({
                    "type": "reasoning", "id": "rs_1", "summary": [],
                    "encrypted_content": "opaque-reasoning", "future": "retained"
                }))
                .unwrap(),
            ),
        ],
    });
    let snapshot = Snapshot::for_compaction(
        parent.clone(),
        &client(),
        ContextLimits::new(16_000, 2048).unwrap(),
        Duration::from_secs(1),
    )
    .unwrap();
    let question = snapshot
        .question_request("What was retained?".into())
        .unwrap();
    let parent = clients::openai::ClientRequest::try_from(parent).unwrap();
    let question = clients::openai::ClientRequest::try_from(question).unwrap();
    assert_eq!(
        serde_json::to_value(&question.input[..parent.input.len()]).unwrap(),
        serde_json::to_value(&parent.input).unwrap()
    );
    assert_eq!(question.instructions, parent.instructions);
    assert_eq!(question.prompt_cache_key, parent.prompt_cache_key);
}

#[test]
fn compaction_snapshot_uses_the_parent_window_and_a_smaller_response_reserve() {
    let request = ClientRequest::new(vec![Message::new("full context ".repeat(20_000))]);
    let limits = ContextLimits::new(24_000, 2048).unwrap();
    assert!(Snapshot::new(request.clone(), &client(), limits, Duration::from_secs(1)).is_err());
    assert!(Snapshot::for_compaction(request, &client(), limits, Duration::from_secs(1)).is_err());
    let snapshot =
        Snapshot::for_compaction(context(), &client(), limits, Duration::from_secs(1)).unwrap();
    assert_eq!(snapshot.limits.ceiling(), limits.ceiling());
    assert_eq!(snapshot.limits.response(), 2048);

    let snapshot = Snapshot::for_compaction(
        context(),
        &client(),
        ContextLimits::new(64_000, 16_000).unwrap(),
        Duration::from_secs(1),
    )
    .unwrap();
    assert_eq!(snapshot.limits.response(), 4096);
    assert_eq!(snapshot.context.request().max_output_tokens, Some(4096));
}

#[test]
fn compaction_snapshot_inherits_a_parent_context_override() {
    let limits = ContextLimits::new(256_000, 16_000).unwrap();
    let request = ClientRequest::new(vec![Message::new("full context ".repeat(70_000))]);
    let snapshot =
        Snapshot::for_compaction(request, &client(), limits, Duration::from_secs(1)).unwrap();
    let request = snapshot
        .question_request("What was captured?".into())
        .unwrap();
    assert!(estimated_tokens(&request).unwrap() > client().context_window());
    assert!(estimated_tokens(&request).unwrap() <= snapshot.limits.input());
    assert_eq!(snapshot.limits.ceiling(), limits.ceiling());
    assert_eq!(snapshot.limits.response(), 4096);
}

#[test]
fn empty_context_and_zero_timeout_are_rejected() {
    for (request, timeout) in [
        (ClientRequest::new(Vec::new()), Duration::from_secs(1)),
        (context(), Duration::ZERO),
    ] {
        assert!(
            Snapshot::new(
                request,
                &client(),
                ContextLimits::new(4096, 1024).unwrap(),
                timeout,
            )
            .is_err()
        );
    }
}

#[test]
fn model_override_determines_the_frozen_context_budget() {
    use clients::config::{Config, ConfigContext};
    use clients::{ClaudeAuthConfig, ClaudeConfig, ClaudeEffort, ClaudeKeyConfig};

    for (configured, requested) in [
        ("claude-opus-4-7", "claude-haiku-4-5"),
        ("claude-haiku-4-5", "claude-opus-4-7"),
    ] {
        let config = Config::Claude(ClaudeConfig {
            auth: ClaudeAuthConfig::APIKey(ClaudeKeyConfig {
                api_key: "fixture-key".into(),
            }),
            model: configured.into(),
            effort: ClaudeEffort::Low,
        });
        let source_config = ConfigContext::new(config.clone());
        let client = LLmClient::new(source_config.clone()).unwrap();
        let request = ClientRequest {
            model: Some(requested.into()),
            ..context()
        };
        let snapshot = Snapshot::new(
            request,
            &client,
            ContextLimits::new(2_000_000, 1024).unwrap(),
            Duration::from_secs(1),
        )
        .unwrap();
        assert_eq!(
            snapshot.limits.ceiling(),
            clients::models::context_window(requested)
        );
        assert_eq!(snapshot.context.request().model.as_deref(), Some(requested));
        assert!(
            matches!(&snapshot.client, LLmClient::Claude { config: captured, .. } if captured.get_config() == config)
        );
        assert_eq!(source_config.get_config(), config);
    }
}
