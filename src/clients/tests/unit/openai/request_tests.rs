use super::*;
use crate::{LocalOpenAIConfig, OpenAICodexConfig, OpenAIKeyConfig, OpenRouterConfig};
use serde_json::json;

#[test]
fn tool_properties_keep_canonical_serialization_across_map_layouts() {
    let property = ToolProperty::Schema(json!({"type": "string"}));
    let mut first = FnvHashMap::default();
    first.insert("zebra".into(), property.clone());
    first.insert("alpha".into(), property.clone());
    let mut second = FnvHashMap::default();
    second.reserve(64);
    second.insert("alpha".into(), property.clone());
    second.insert("zebra".into(), property);
    let parameters = |properties| FunctionParameters {
        param_type: "object".into(),
        properties,
        required: Vec::new(),
    };
    let encoded = serde_json::to_string(&parameters(first.clone())).unwrap();
    assert_eq!(
        encoded,
        serde_json::to_string(&parameters(second.clone())).unwrap()
    );
    assert_eq!(
        encoded,
        r#"{"type":"object","properties":{"alpha":{"type":"string"},"zebra":{"type":"string"}},"required":[]}"#
    );
    let object = |properties| ToolProperty::Object {
        name: "nested".into(),
        prop_type: "object".into(),
        description: "Nested properties".into(),
        properties,
    };
    assert_eq!(
        serde_json::to_string(&object(first)).unwrap(),
        serde_json::to_string(&object(second)).unwrap()
    );
}

#[path = "cache_tests.rs"]
mod cache_tests;

#[path = "routing_tests.rs"]
mod routing_tests;

#[path = "sse_tests.rs"]
mod sse_tests;

fn config(auth: OpenAIAuthConfig) -> OpenAIConfig {
    OpenAIConfig {
        auth,
        model: "fixture".into(),
        effort: OpenAIEffort::Low,
        request_encrypted_reasoning: None,
        fast_mode: Default::default(),
    }
}

fn wire_request(config: &OpenAIConfig, mode: ResponseMode) -> serde_json::Value {
    serde_json::to_value(ResponseRequest::new(
        config,
        ClientRequest::new(vec![InputItem::user("task".into())])
            .with_instructions("operating instructions".into()),
        mode,
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
fn fast_mode_selects_fast_or_standard_tiers_in_both_request_modes() {
    use crate::{FastMode, config::Config};

    for auth in [
        codex_auth(),
        OpenAIAuthConfig::APIKey(OpenAIKeyConfig {
            api_key: "fixture".into(),
            url: None,
        }),
        OpenAIAuthConfig::APIKey(OpenAIKeyConfig {
            api_key: "fixture".into(),
            url: Some("https://api.openai.com/v1/".into()),
        }),
    ] {
        let mut provider = Config::OpenAI(config(auth));
        assert_eq!(provider.toggle_fast_mode().unwrap(), FastMode::Enabled);
        let Config::OpenAI(enabled) = provider.clone() else {
            panic!("Expected OpenAI config")
        };
        for mode in [ResponseMode::Complete, ResponseMode::Streaming] {
            let body = wire_request(&enabled, mode);
            assert_eq!(body["service_tier"], "priority");
            assert_eq!(body["model"], "fixture");
            assert_eq!(body["reasoning"]["effort"], "low");
        }
        assert_eq!(provider.toggle_fast_mode().unwrap(), FastMode::Disabled);
        let Config::OpenAI(disabled) = provider else {
            panic!("Expected OpenAI config")
        };
        for mode in [ResponseMode::Complete, ResponseMode::Streaming] {
            let body = wire_request(&disabled, mode);
            match disabled.auth {
                OpenAIAuthConfig::Codex(_) => assert!(body.get("service_tier").is_none()),
                _ => assert_eq!(body["service_tier"], "default"),
            }
        }
    }
}

#[test]
fn fast_mode_never_leaks_service_tiers_to_compatible_endpoints() {
    for auth in [
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
    ] {
        let mut config = config(auth);
        for mode in [crate::FastMode::Disabled, crate::FastMode::Enabled] {
            config.fast_mode = mode;
            for request_mode in [ResponseMode::Complete, ResponseMode::Streaming] {
                assert!(
                    wire_request(&config, request_mode)
                        .get("service_tier")
                        .is_none()
                );
            }
        }
    }
}

#[test]
fn fast_mode_is_applied_to_codex_compaction_requests() {
    let mut config = config(codex_auth());
    config.fast_mode = crate::FastMode::Enabled;
    let request = ResponseRequest::compaction(
        &config,
        ClientRequest::new(vec![InputItem::user("task".into())]).with_model("gpt-6-astra".into()),
    );
    let client = OpenAIClient::new(config).unwrap();
    assert_eq!(
        client.response_headers(&request).unwrap()["x-codex-routing-hint"],
        "model=gpt-6-astra;tier=priority"
    );
    let body = serde_json::to_value(request).unwrap();
    assert_eq!(body["service_tier"], "priority");
}

#[test]
fn codex_compaction_appends_a_transient_trigger_to_a_streaming_request() {
    let config = config(codex_auth());
    let previous =
        json!({"type":"compaction", "encrypted_content":"opaque", "future":{"keep":true}});
    let mut request = ClientRequest::new(vec![
        InputItem::Native(previous.clone()),
        InputItem::user("latest requirement".into()),
    ])
    .with_model("gpt-6-astra".into())
    .with_instructions("current instructions".into());
    request.prompt_cache_key = Some("session-1".into());
    let body = serde_json::to_value(ResponseRequest::compaction(&config, request)).unwrap();
    assert_eq!(body["prompt_cache_key"], "session-1");
    assert_eq!(body["include"], json!(["reasoning.encrypted_content"]));
    assert_eq!(body["model"], "gpt-6-astra");
    assert_eq!(body["instructions"], "current instructions");
    assert_eq!(body["stream"], true);
    assert_eq!(body["store"], false);
    assert!(body.get("max_output_tokens").is_none());
    assert!(body.get("prompt_cache_options").is_none());
    assert_eq!(
        body["input"],
        json!([
            previous,
            {"type":"message", "role":"user", "content":"latest requirement"},
            {"type":"compaction_trigger"},
        ])
    );
    assert_eq!(
        wire_request(&config, ResponseMode::Streaming)["input"]
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
    for mode in [ResponseMode::Complete, ResponseMode::Streaming] {
        let request = wire_request(&config, mode);
        match mode {
            ResponseMode::Complete => assert!(request.get("stream").is_none()),
            ResponseMode::Streaming => assert_eq!(request["stream"], true),
        }
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
    ];
    for route in routes {
        let mut config = config(route);
        assert!(
            wire_request(&config, ResponseMode::Streaming)
                .get("include")
                .is_none()
        );
        config.request_encrypted_reasoning = Some(true);
        assert_eq!(
            wire_request(&config, ResponseMode::Streaming)["include"],
            json!(["reasoning.encrypted_content"])
        );
        config.request_encrypted_reasoning = Some(false);
        assert!(
            wire_request(&config, ResponseMode::Streaming)
                .get("include")
                .is_none()
        );
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

#[test]
fn cache_keys_follow_supported_routes_and_codex_reasoning_can_be_disabled() {
    for auth in [
        codex_auth(),
        OpenAIAuthConfig::APIKey(OpenAIKeyConfig {
            api_key: "fixture".into(),
            url: None,
        }),
        OpenAIAuthConfig::APIKey(OpenAIKeyConfig {
            api_key: "fixture".into(),
            url: Some("https://api.openai.com/v1/".into()),
        }),
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
    ] {
        let mut config = config(auth);
        let supported = matches!(config.auth, OpenAIAuthConfig::Codex(_))
            || config.get_url().starts_with("https://api.openai.com/");
        let request = llm::ClientRequest::new(vec![llm::Message::new("task".into())])
            .with_prompt_cache_key(Some("session-1".into()))
            .with_model("gpt-6-astra".into())
            .with_thinking()
            .with_tools(vec![])
            .with_output_limit(4096);
        for request in [request.clone(), request] {
            let body = serde_json::to_value(ResponseRequest::new(
                &config,
                request.try_into().unwrap(),
                ResponseMode::Streaming,
            ))
            .unwrap();
            assert_eq!(
                body.get("prompt_cache_key"),
                supported.then_some(&json!("session-1"))
            );
            assert_eq!(body.get("include").is_some(), supported);
            assert_eq!(body["model"], "gpt-6-astra");
        }
        config.request_encrypted_reasoning = Some(false);
        assert!(
            wire_request(&config, ResponseMode::Streaming)
                .get("include")
                .is_none()
        );
        assert!(
            wire_request(&config, ResponseMode::Streaming)
                .get("prompt_cache_key")
                .is_none()
        );
    }
}

#[tokio::test]
async fn codex_quota_exhaustion_sends_exactly_one_http_request() {
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
    let fixture = crate::http_fixture::HttpFixture::new().await;
    let listener = fixture.listener.clone();
    let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count = calls.clone();
    let server = tokio::spawn(async move {
        while let Ok((socket, _)) = listener.accept().await {
            count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let mut reader = BufReader::new(socket);
            let mut line = String::new();
            let mut length = 0;
            while line != "\r\n" {
                line.clear();
                assert!(reader.read_line(&mut line).await.unwrap() > 0);
                if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = value.trim().parse::<usize>().unwrap();
                }
            }
            reader.read_exact(&mut vec![0; length]).await.unwrap();
            let body = r#"{"error":{"type":"usage_limit_reached","message":"Usage exhausted","resets_at":1800000000,"resets_in_seconds":7200}}"#;
            let wire = format!(
                "HTTP/1.1 429 Too Many Requests\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            reader.get_mut().write_all(wire.as_bytes()).await.unwrap();
        }
    });
    let mut client = fixture.client(config(codex_auth()));
    client.config.auth = OpenAIAuthConfig::Local(LocalOpenAIConfig {
        api_key: None,
        url: fixture.url.clone(),
    });
    let result = tokio::time::timeout(
        Duration::from_secs(2),
        client.chat_stream_openai(ClientRequest::new(vec![InputItem::user("task".into())])),
    )
    .await
    .unwrap();
    let error = result.err().unwrap();
    let failure = error.downcast_ref::<crate::failure::Failure>().unwrap();
    assert_eq!(failure.kind, crate::failure::FailureKind::UsageLimit);
    assert!(!failure.retryable());
    assert!(failure.message.contains("1800000000"));
    assert!(failure.message.contains("7200"));
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    server.abort();
    assert!(server.await.unwrap_err().is_cancelled());
}
