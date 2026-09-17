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
                true,
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
        assert!(wire_request(&config, true).get("include").is_none());
        assert!(
            wire_request(&config, true)
                .get("prompt_cache_key")
                .is_none()
        );
    }
}

#[tokio::test]
async fn codex_quota_exhaustion_sends_exactly_one_http_request() {
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
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
    let mut client = OpenAIClient::new(config(codex_auth())).unwrap();
    client.config.auth = OpenAIAuthConfig::Local(LocalOpenAIConfig { api_key: None, url });
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
