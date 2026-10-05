use super::*;
use crate::{LocalOpenAIConfig, OpenAIAuthConfig, llm::LLmClient};

fn openai_config() -> Config {
    serde_json::from_value(serde_json::json!({
        "OpenAI": {
            "auth": {"APIKey": {"api_key": "fixture", "url": null}},
            "model": "fixture",
            "effort": "high"
        }
    }))
    .unwrap()
}

#[test]
fn fast_mode_defaults_off_and_toggle_round_trips_without_changing_model_or_effort() {
    let mut config = openai_config();
    let original = config.clone();
    assert_eq!(config.fast_mode(), FastMode::Disabled);
    assert_eq!(config.toggle_fast_mode().unwrap(), FastMode::Enabled);
    assert_eq!(config.get_model(), original.get_model());
    assert_eq!(config.get_effort(), original.get_effort());
    let saved = toml::to_string_pretty(&config).unwrap();
    let mut restored: Config = toml::from_str(&saved).unwrap();
    assert_eq!(restored, config);
    assert_eq!(restored.toggle_fast_mode().unwrap(), FastMode::Disabled);
    assert_eq!(restored, original);
}

#[test]
fn fast_mode_rejects_unsupported_providers_without_mutating_configuration() {
    let Config::OpenAI(base) = openai_config() else {
        panic!("Expected OpenAI config")
    };
    let claude = Config::Claude(ClaudeConfig {
        auth: crate::ClaudeAuthConfig::APIKey(crate::ClaudeKeyConfig {
            api_key: "fixture".into(),
        }),
        model: "fixture".into(),
        effort: ClaudeEffort::High,
    });
    let configs = [
        claude,
        Config::OpenAI(OpenAIConfig {
            auth: OpenAIAuthConfig::APIKey(crate::OpenAIKeyConfig {
                api_key: "fixture".into(),
                url: Some("https://compatible.invalid/v1".into()),
            }),
            ..base.clone()
        }),
        Config::OpenAI(OpenAIConfig {
            auth: OpenAIAuthConfig::Local(LocalOpenAIConfig {
                api_key: None,
                url: "http://localhost:1234/v1".into(),
            }),
            ..base.clone()
        }),
        Config::OpenAI(OpenAIConfig {
            auth: OpenAIAuthConfig::OpenRouter(crate::OpenRouterConfig {
                api_key: "fixture".into(),
                url: None,
            }),
            ..base
        }),
    ];
    for mut config in configs {
        let original = config.clone();
        assert!(config.toggle_fast_mode().is_err());
        assert_eq!(config, original);
    }
}

#[test]
fn shared_clients_follow_fast_toggles_while_active_snapshots_keep_their_mode() {
    let context = ConfigContext::new(openai_config());
    let client = LLmClient::new(context.clone()).unwrap();
    let child = client.clone();
    let active = client.snapshot();
    assert_eq!(
        context.config.lock().unwrap().toggle_fast_mode().unwrap(),
        FastMode::Enabled
    );
    for inherited in [client, child] {
        let LLmClient::OpenApi { config, .. } = inherited else {
            panic!("Expected an OpenAI client")
        };
        assert_eq!(config.get_config().fast_mode(), FastMode::Enabled);
    }
    let LLmClient::OpenApi { config, .. } = active else {
        panic!("Expected an OpenAI snapshot")
    };
    assert_eq!(config.get_config().fast_mode(), FastMode::Disabled);
}

#[test]
fn shared_clients_follow_model_changes_while_active_requests_keep_their_model() {
    let context = ConfigContext::new(Config::OpenAI(OpenAIConfig {
        auth: OpenAIAuthConfig::Local(LocalOpenAIConfig {
            api_key: None,
            url: "http://localhost:1234/v1".into(),
        }),
        model: "anthropic/claude-opus-4-7".into(),
        effort: OpenAIEffort::High,
        request_encrypted_reasoning: None,
        fast_mode: Default::default(),
    }));
    let client = LLmClient::new(context.clone()).unwrap();
    let child = client.clone();
    let active = client.snapshot();
    assert_eq!(client.context_window(), 1_000_000);
    context
        .config
        .lock()
        .unwrap()
        .set_model("anthropic/claude-haiku-4-5-20251001".into());
    assert_eq!(client.context_window(), 200_000);
    assert_eq!(child.context_window(), 200_000);
    assert_eq!(active.context_window(), 1_000_000);
    let LLmClient::OpenApi { config, .. } = active else {
        panic!("The active request should keep its provider")
    };
    assert_eq!(config.get_config().get_model(), "anthropic/claude-opus-4-7");
    context
        .config
        .lock()
        .unwrap()
        .set_model("custom-model".into());
    assert_eq!(
        child.context_window(),
        crate::models::FALLBACK_CONTEXT_WINDOW
    );
}
