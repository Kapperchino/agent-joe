use super::*;
use crate::{LocalOpenAIConfig, OpenAICodexConfig, OpenAIKeyConfig, OpenRouterConfig};
use serde_json::json;

fn config(auth: OpenAIAuthConfig) -> OpenAIConfig {
    OpenAIConfig {
        auth,
        model: "fixture".into(),
        effort: OpenAIEffort::Low,
        request_encrypted_reasoning: None,
    }
}

fn wire_request(config: &OpenAIConfig, stream: bool) -> serde_json::Value {
    serde_json::to_value(ResponseRequest::new(
        config,
        ClientRequest::new(vec![InputItem::user("task".into())])
            .with_instructions("operating instructions".into()),
        stream,
    ))
    .unwrap()
}

fn codex_auth() -> OpenAIAuthConfig {
    OpenAIAuthConfig::Codex(OpenAICodexConfig {
        id_token: "fixture".into(),
        access_token: "fixture".into(),
        refresh_token: "fixture".into(),
        account_id: "fixture".into(),
        last_refresh: Duration::ZERO,
        expires_at_ms: 0,
    })
}

#[test]
fn codex_compaction_appends_a_transient_trigger_to_a_streaming_request() {
    let config = config(codex_auth());
    let previous =
        json!({"type":"compaction", "encrypted_content":"opaque", "future":{"keep":true}});
    let request = ClientRequest::new(vec![
        InputItem::Native(previous.clone()),
        InputItem::user("latest requirement".into()),
    ])
    .with_model("gpt-6-astra".into())
    .with_instructions("current instructions".into());
    let body = serde_json::to_value(ResponseRequest::compaction(&config, request)).unwrap();
    assert_eq!(body["model"], "gpt-6-astra");
    assert_eq!(body["instructions"], "current instructions");
    assert_eq!(body["stream"], true);
    assert_eq!(body["store"], false);
    assert!(body.get("max_output_tokens").is_none());
    assert_eq!(
        body["input"],
        json!([
            previous,
            {"type":"message", "role":"user", "content":"latest requirement"},
            {"type":"compaction_trigger"},
        ])
    );
    assert_eq!(
        wire_request(&config, true)["input"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn native_compaction_auto_recognizes_codex_and_public_openai_only() {
    use crate::{
        config::{Config, ConfigContext},
        llm::LLmClient,
    };
    struct Route {
        auth: OpenAIAuthConfig,
        supported: bool,
    }
    for route in [
        Route {
            auth: codex_auth(),
            supported: true,
        },
        Route {
            auth: OpenAIAuthConfig::APIKey(OpenAIKeyConfig {
                api_key: "fixture".into(),
                url: None,
            }),
            supported: true,
        },
        Route {
            auth: OpenAIAuthConfig::APIKey(OpenAIKeyConfig {
                api_key: "fixture".into(),
                url: Some("https://api.openai.com/v1/".into()),
            }),
            supported: true,
        },
        Route {
            auth: OpenAIAuthConfig::APIKey(OpenAIKeyConfig {
                api_key: "fixture".into(),
                url: Some("https://compatible.invalid/v1".into()),
            }),
            supported: false,
        },
        Route {
            auth: OpenAIAuthConfig::Local(LocalOpenAIConfig {
                api_key: None,
                url: "http://localhost:1234/v1".into(),
            }),
            supported: false,
        },
        Route {
            auth: OpenAIAuthConfig::OpenRouter(OpenRouterConfig {
                api_key: "fixture".into(),
                url: None,
            }),
            supported: false,
        },
    ] {
        let client =
            LLmClient::new(ConfigContext::new(Config::OpenAI(config(route.auth)))).unwrap();
        assert_eq!(client.native_compaction(), route.supported);
    }
}

#[test]
fn public_openai_requests_encrypted_state_in_streaming_and_nonstreaming_requests() {
    let config = config(OpenAIAuthConfig::APIKey(OpenAIKeyConfig {
        api_key: "fixture".into(),
        url: None,
    }));
    for stream in [false, true] {
        let request = wire_request(&config, stream);
        assert_eq!(request["include"], json!(["reasoning.encrypted_content"]));
        assert_eq!(request["store"], false);
        assert_eq!(request["instructions"], "operating instructions");
        assert_eq!(
            request["input"],
            json!([{"type":"message","role":"user","content":"task"}])
        );
    }
}

#[test]
fn compatible_routes_can_opt_in_without_receiving_unknown_fields_by_default() {
    let routes = [
        OpenAIAuthConfig::APIKey(OpenAIKeyConfig {
            api_key: "fixture".into(),
            url: Some("https://compatible.invalid/v1".into()),
        }),
        OpenAIAuthConfig::Local(LocalOpenAIConfig {
            api_key: None,
            url: "http://localhost:1234/v1".into(),
        }),
        OpenAIAuthConfig::OpenRouter(OpenRouterConfig {
            api_key: "fixture".into(),
            url: None,
        }),
        OpenAIAuthConfig::Codex(OpenAICodexConfig {
            id_token: "fixture".into(),
            access_token: "fixture".into(),
            refresh_token: "fixture".into(),
            account_id: "fixture".into(),
            last_refresh: Duration::ZERO,
            expires_at_ms: 0,
        }),
    ];
    for route in routes {
        let mut config = config(route);
        assert!(wire_request(&config, true).get("include").is_none());
        config.request_encrypted_reasoning = Some(true);
        assert_eq!(
            wire_request(&config, true)["include"],
            json!(["reasoning.encrypted_content"])
        );
        config.request_encrypted_reasoning = Some(false);
        assert!(wire_request(&config, true).get("include").is_none());
    }
}

#[test]
fn old_provider_config_remains_readable() {
    let config: OpenAIConfig = serde_json::from_value(json!({
        "auth":{"APIKey":{"api_key":"fixture","url":null}},
        "model":"fixture","effort":"low"
    }))
    .unwrap();
    assert_eq!(config.request_encrypted_reasoning, None);
    assert_eq!(
        config.reasoning_include(),
        vec![ResponseInclude::EncryptedReasoning]
    );
}
