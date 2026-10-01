use super::*;
use crate::claude_config::ClaudeKeyConfig;
use serde_json::json;

fn config(model: &str) -> ClaudeConfig {
    ClaudeConfig {
        auth: ClaudeAuthConfig::APIKey(ClaudeKeyConfig {
            api_key: "fixture".into(),
        }),
        model: model.into(),
        effort: ClaudeEffort::Med,
    }
}

fn request() -> llm::ClientRequest {
    llm::ClientRequest::new(vec![llm::Message::new("task".into())])
        .with_system("instructions".into())
        .with_output_limit(16_000)
}

#[test]
fn new_models_use_adaptive_thinking_and_nested_effort_in_both_request_modes() {
    for model in ["claude-opus-5-5", "claude-fable-5-1", "claude-sonnet-5-5"] {
        let config = config(model);
        for input in [request(), request().with_thinking()] {
            let body =
                serde_json::to_value(ChatRequest::new(input.try_into().unwrap(), &config)).unwrap();
            assert_eq!(body["model"], model);
            assert_eq!(body["thinking"], json!({"type": "adaptive"}));
            assert_eq!(body["output_config"], json!({"effort": "medium"}));
            assert_eq!(body["max_tokens"], 16_000);
            assert_eq!(body["system"], "instructions");
            assert_eq!(body["messages"][0]["content"][0]["text"], "task");
            assert!(body.get("effort").is_none());
            assert!(body.get("stream").is_none());
        }
        let body = serde_json::to_value(ChatRequestStream {
            request: ChatRequest::new(request().with_thinking().try_into().unwrap(), &config),
            stream: true,
        })
        .unwrap();
        assert_eq!(body["model"], model);
        assert_eq!(body["thinking"], json!({"type": "adaptive"}));
        assert_eq!(body["output_config"], json!({"effort": "medium"}));
        assert_eq!(body["stream"], true);
        assert!(body.get("effort").is_none());
        assert!(body.get("request").is_none());
    }
}

#[test]
fn request_model_and_effort_overrides_take_precedence() {
    let mut input: ClientRequest = request()
        .with_model("claude-fable-5-1".into())
        .with_output_limit(512)
        .try_into()
        .unwrap();
    input.effort = Some(ClaudeEffort::Xhigh);
    let body = serde_json::to_value(ChatRequest::new(input, &config("claude-haiku-4-5"))).unwrap();
    assert_eq!(body["model"], "claude-fable-5-1");
    assert_eq!(body["max_tokens"], 512);
    assert_eq!(body["thinking"], json!({"type": "adaptive"}));
    assert_eq!(body["output_config"], json!({"effort": "xhigh"}));
}

#[test]
fn earlier_adaptive_models_do_not_send_manual_thinking_budgets() {
    for model in ["claude-opus-4-7", "claude-sonnet-4-6"] {
        let body = serde_json::to_value(ChatRequest::new(
            request().with_thinking().try_into().unwrap(),
            &config(model),
        ))
        .unwrap();
        assert_eq!(body["thinking"], json!({"type": "adaptive"}));
        assert_eq!(body["output_config"], json!({"effort": "medium"}));
    }
}

#[test]
fn legacy_models_retain_manual_thinking_without_unsupported_effort() {
    for model in ["claude-haiku-4-5", "claude-sonnet-4-20250514"] {
        let body = serde_json::to_value(ChatRequest::new(
            request().with_thinking().try_into().unwrap(),
            &config(model),
        ))
        .unwrap();
        assert_eq!(
            body["thinking"],
            json!({"type": "enabled", "budget_tokens": 1024})
        );
        assert!(body.get("output_config").is_none());
        for input in [request(), request().with_thinking().with_output_limit(1024)] {
            let body =
                serde_json::to_value(ChatRequest::new(input.try_into().unwrap(), &config(model)))
                    .unwrap();
            assert!(body.get("thinking").is_none());
            assert!(body.get("output_config").is_none());
        }
    }
}

#[test]
fn medium_effort_serializes_for_the_api_and_accepts_existing_configs() {
    assert_eq!(serde_json::to_value(ClaudeEffort::Med).unwrap(), "medium");
    for value in ["med", "medium"] {
        assert_eq!(
            serde_json::from_value::<ClaudeEffort>(json!(value)).unwrap(),
            ClaudeEffort::Med
        );
    }
}
