use super::*;

#[test]
fn aliases_resolve_without_matching_unknown_model_names() {
    assert_eq!(context_window("gpt-5.6"), context_window("gpt-5.6-sol"));
    assert_eq!(context_window("openai/gpt-5.4-2026-03-05"), 1_050_000);
    assert_eq!(context_window("openai/gpt-5"), 400_000);
    assert_eq!(context_window("claude-sonnet-4-20250514"), 200_000);
    assert_eq!(codex_context_window("gpt-5.6"), 272_000);
    assert_eq!(context_window("gpt-5.4-custom"), FALLBACK_CONTEXT_WINDOW);
    assert_eq!(context_window("custom/gpt-5.4"), FALLBACK_CONTEXT_WINDOW);
}
