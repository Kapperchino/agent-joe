use super::*;
use crate::config::{Config, ConfigContext};
use crate::llm::LLmClient;

fn headers(state: &'static str) -> header::HeaderMap {
    header::HeaderMap::from_iter([(
        header::HeaderName::from_static("x-codex-turn-state"),
        header::HeaderValue::from_static(state),
    )])
}

fn openai(client: &LLmClient) -> &OpenAIClient {
    match client {
        LLmClient::OpenApi { client, .. } => client,
        _ => panic!("Expected an OpenAI client"),
    }
}

#[test]
fn codex_snapshots_share_first_routing_state_only_within_the_same_turn() {
    let mut client =
        LLmClient::new(ConfigContext::new(Config::OpenAI(config(codex_auth())))).unwrap();
    client.begin_turn();
    let first = client.snapshot();
    let initial = openai(&first).routing_headers(Some("session-1")).unwrap();
    assert_eq!(initial["session-id"], "session-1");
    assert!(!initial.contains_key("x-codex-turn-state"));
    openai(&first).observe_routing(Some("session-1"), &header::HeaderMap::new());
    openai(&first).observe_routing(Some("session-1"), &headers("route-1"));
    let continuation = client.snapshot();
    assert_eq!(
        openai(&continuation)
            .routing_headers(Some("session-1"))
            .unwrap()["x-codex-turn-state"],
        "route-1"
    );
    openai(&continuation).observe_routing(Some("session-1"), &headers("replacement"));
    assert_eq!(
        openai(&client).routing_headers(Some("session-1")).unwrap()["x-codex-turn-state"],
        "route-1"
    );
    assert!(
        !openai(&client)
            .routing_headers(Some("worker-1"))
            .unwrap()
            .contains_key("x-codex-turn-state")
    );
    let mut worker = client.snapshot();
    worker.begin_turn();
    openai(&worker).observe_routing(Some("worker-1"), &headers("worker-route"));
    assert_eq!(
        openai(&client).routing_headers(Some("session-1")).unwrap()["x-codex-turn-state"],
        "route-1"
    );
    client.begin_turn();
    assert!(
        !openai(&client)
            .routing_headers(Some("session-1"))
            .unwrap()
            .contains_key("x-codex-turn-state")
    );
    openai(&first).observe_routing(Some("session-1"), &headers("late-response"));
    openai(&client).observe_routing(Some("session-1"), &headers("route-2"));
    assert_eq!(
        openai(&client).routing_headers(Some("session-1")).unwrap()["x-codex-turn-state"],
        "route-2"
    );
    assert!(openai(&client).routing_headers(None).unwrap().is_empty());
    assert!(
        openai(&client)
            .routing_headers(Some("invalid\nheader"))
            .is_err()
    );
}

#[test]
fn codex_routing_headers_do_not_leak_to_other_providers() {
    for auth in [
        OpenAIAuthConfig::APIKey(OpenAIKeyConfig {
            api_key: "fixture".into(),
            url: None,
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
        let client = OpenAIClient::new(config(auth)).unwrap();
        client.observe_routing(Some("session-1"), &headers("route-1"));
        assert!(
            client
                .routing_headers(Some("session-1"))
                .unwrap()
                .is_empty()
        );
        assert!(
            client
                .routing_headers(Some("invalid\nheader"))
                .unwrap()
                .is_empty()
        );
    }
}
