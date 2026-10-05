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
        let request = ResponseRequest::new(
            &client.config,
            ClientRequest::new(vec![]).with_model("invalid\nmodel".into()),
            ResponseMode::Streaming,
        );
        assert!(client.response_headers(&request).unwrap().is_empty());
    }
}

#[test]
fn codex_routing_hint_tracks_request_model_and_fast_toggles_with_pinned_turn_state() {
    let mut client = OpenAIClient::new(config(codex_auth())).unwrap();
    client.observe_routing(Some("session-1"), &headers("route-1"));
    for mode in [crate::FastMode::Enabled, crate::FastMode::Disabled] {
        client.config.fast_mode = mode;
        let expected = match mode {
            crate::FastMode::Enabled => "model=gpt-6-astra;tier=priority",
            crate::FastMode::Disabled => "model=gpt-6-astra",
        };
        for response_mode in [ResponseMode::Complete, ResponseMode::Streaming] {
            let mut input = ClientRequest::new(vec![]).with_model("gpt-6-astra".into());
            input.prompt_cache_key = Some("session-1".into());
            let request = ResponseRequest::new(&client.config, input, response_mode);
            let actual = client.response_headers(&request).unwrap();
            assert_eq!(actual["x-codex-routing-hint"], expected);
            assert_eq!(actual["session-id"], "session-1");
            assert_eq!(actual["x-codex-turn-state"], "route-1");
        }
    }
}

#[test]
fn codex_routing_hint_does_not_require_a_cache_key_and_rejects_invalid_models() {
    let client = OpenAIClient::new(config(codex_auth())).unwrap();
    let mut request = ResponseRequest::new(
        &client.config,
        ClientRequest::new(vec![]),
        ResponseMode::Streaming,
    );
    let actual = client.response_headers(&request).unwrap();
    assert_eq!(actual["x-codex-routing-hint"], "model=fixture");
    assert!(!actual.contains_key("session-id"));
    request.service_tier = Some("priority");
    assert_eq!(
        client.response_headers(&request).unwrap()["x-codex-routing-hint"],
        "model=fixture;tier=priority"
    );
    request.model = "invalid\nmodel".into();
    assert!(client.response_headers(&request).is_err());
}
