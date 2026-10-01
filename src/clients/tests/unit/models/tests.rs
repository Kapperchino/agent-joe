use super::*;
use crate::{ClaudeEffort, OpenAIEffort};
use strum::VariantNames;

#[test]
fn new_gpt_models_are_selectable_with_api_and_codex_context_limits() {
    for name in ["gpt-6.1-sol", "gpt-6-sol", "gpt-6-luna"] {
        assert!(OpenAIModels::VARIANTS.contains(&name));
        assert_eq!(
            name.parse::<OpenAIModels>().unwrap().context_window(),
            1_050_000
        );
        for model in [name.to_owned(), format!("openai/{name}")] {
            assert_eq!(context_window(&model), 1_050_000, "{model}");
            assert_eq!(codex_context_window(&model), 272_000, "{model}");
        }
    }
}

#[test]
fn new_claude_models_are_selectable_with_million_token_contexts() {
    for name in ["claude-opus-5-5", "claude-fable-5-1", "claude-sonnet-5-5"] {
        assert!(ClaudeModels::VARIANTS.contains(&name));
        assert_eq!(
            name.parse::<ClaudeModels>().unwrap().context_window(),
            1_000_000
        );
        assert_eq!(context_window(name), 1_000_000);
        assert_eq!(context_window(&format!("anthropic/{name}")), 1_000_000);
        assert_eq!(codex_context_window(name), FALLBACK_CONTEXT_WINDOW);
    }
}

#[test]
fn gpt_efforts_distinguish_required_and_optional_reasoning() {
    for name in ["gpt-6.1-sol", "gpt-6-astra"] {
        for model in [name.to_owned(), format!("openai/{name}")] {
            assert_eq!(
                OpenAIEffort::supported_for_model(&model),
                &[
                    OpenAIEffort::Low,
                    OpenAIEffort::Medium,
                    OpenAIEffort::High,
                    OpenAIEffort::Xhigh,
                    OpenAIEffort::Max,
                ],
                "{model}"
            );
        }
    }
    for name in ["gpt-6-sol", "gpt-6-luna", "gpt-5.6"] {
        for model in [name.to_owned(), format!("openai/{name}")] {
            assert_eq!(
                OpenAIEffort::supported_for_model(&model),
                &[
                    OpenAIEffort::None,
                    OpenAIEffort::Low,
                    OpenAIEffort::Medium,
                    OpenAIEffort::High,
                    OpenAIEffort::Xhigh,
                    OpenAIEffort::Max,
                ],
                "{model}"
            );
        }
    }
}

#[test]
fn claude_efforts_include_xhigh_only_for_supported_models() {
    for name in [
        "claude-opus-5-5",
        "claude-fable-5-1",
        "claude-sonnet-5-5",
        "claude-opus-4-7",
    ] {
        for model in [name.to_owned(), format!("anthropic/{name}")] {
            assert_eq!(
                ClaudeEffort::supported_for_model(&model),
                &[
                    ClaudeEffort::Low,
                    ClaudeEffort::Med,
                    ClaudeEffort::High,
                    ClaudeEffort::Xhigh,
                    ClaudeEffort::Max,
                ]
            );
        }
    }
    assert!(!ClaudeEffort::supported_for_model("claude-sonnet-4-6").contains(&ClaudeEffort::Xhigh));
}

#[test]
fn aliases_resolve_without_matching_unknown_model_names() {
    assert_eq!(context_window("gpt-5.6"), context_window("gpt-5.6-sol"));
    assert_eq!(context_window("openai/gpt-5.4-2026-03-05"), 1_050_000);
    assert_eq!(context_window("openai/gpt-5"), 400_000);
    assert_eq!(context_window("claude-sonnet-4-20250514"), 200_000);
    assert_eq!(codex_context_window("gpt-5.6"), 272_000);
    assert_eq!(context_window("gpt-5.4-custom"), FALLBACK_CONTEXT_WINDOW);
    assert_eq!(context_window("custom/gpt-5.4"), FALLBACK_CONTEXT_WINDOW);
    assert_eq!(
        context_window("gpt-6.1-sol-custom"),
        FALLBACK_CONTEXT_WINDOW
    );
    assert_eq!(context_window("custom/gpt-6-sol"), FALLBACK_CONTEXT_WINDOW);
    assert_eq!(
        context_window("claude-opus-5-5-custom"),
        FALLBACK_CONTEXT_WINDOW
    );
}
