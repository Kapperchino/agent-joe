use super::*;
use clients::llm::StreamProvider;
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
fn question_budget_reserves_output_and_accepts_exact_boundary() {
    let question = "What did the context say? ".repeat(1000);
    let snapshot = Snapshot::new(
        context(),
        &client(),
        ContextLimits::new(64_000, 1024).unwrap(),
        Duration::from_secs(1),
    )
    .unwrap();
    let required = estimated_tokens(&snapshot.question_request(question.clone()).unwrap()).unwrap();
    for (ceiling, expected) in [(required + 1024, true), (required + 1023, false)] {
        let snapshot = Snapshot::new(
            context(),
            &client(),
            ContextLimits::new(ceiling, 1024).unwrap(),
            Duration::from_secs(1),
        )
        .unwrap();
        assert_eq!(
            snapshot.question_request(question.clone()).is_ok(),
            expected
        );
        assert_eq!(snapshot.request.messages.len(), 1);
    }
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
        assert_eq!(snapshot.request.model.as_deref(), Some(requested));
        assert!(
            matches!(&snapshot.client, LLmClient::Claude { config: captured, .. } if captured.get_config() == config)
        );
        assert_eq!(source_config.get_config(), config);
    }
}

#[tokio::test]
async fn response_byte_limit_counts_all_events() {
    let answer = SnapshotAnswer {
        bytes: MAX_RESPONSE_BYTES - serde_json::to_vec(&StreamEvent::Ping).unwrap().len(),
        ..SnapshotAnswer::new()
    };
    let answer = answer.process(StreamEvent::Ping).await.unwrap();
    assert_eq!(answer.bytes, MAX_RESPONSE_BYTES);
    let error = answer.process(StreamEvent::Ping).await.err().unwrap();
    assert!(error.to_string().contains("response byte limit"));
}
