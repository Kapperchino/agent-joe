use super::*;
use crate::{LocalOpenAIConfig, OpenAIAuthConfig, llm::LLmClient};

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
