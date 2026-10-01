use super::*;

#[test]
fn model_picker_includes_new_and_existing_models() {
    let openai = ModelSelections::OpenAI.get_models();
    for model in [
        "gpt-6.1-sol",
        "gpt-6-astra",
        "gpt-6-sol",
        "gpt-6-luna",
        "gpt-5.6-sol",
        "gpt-5.6-terra",
        "gpt-5.6-luna",
        "gpt-5.5",
        "gpt-5.4",
    ] {
        assert!(openai.iter().any(|item| item == model), "{model}");
    }
    let claude = ModelSelections::Claude.get_models();
    for model in [
        "claude-opus-5-5",
        "claude-fable-5-1",
        "claude-sonnet-5-5",
        "claude-opus-4-7",
        "claude-sonnet-4-6",
        "claude-haiku-4-5",
    ] {
        assert!(claude.iter().any(|item| item == model), "{model}");
    }
    assert!(ModelSelections::Other.get_models().is_empty());
}

#[test]
fn effort_picker_uses_the_selected_model_capabilities() {
    assert_eq!(
        EffortsSelection::OpenAI.get_efforts("gpt-6.1-sol"),
        ["Low", "Medium", "High", "Xhigh", "Max"]
    );
    for model in ["gpt-6-sol", "gpt-6-luna"] {
        assert_eq!(
            EffortsSelection::OpenAI.get_efforts(model),
            ["None", "Low", "Medium", "High", "Xhigh", "Max"]
        );
    }
    for model in ["claude-opus-5-5", "claude-fable-5-1", "claude-sonnet-5-5"] {
        assert_eq!(
            EffortsSelection::Claude.get_efforts(model),
            ["Low", "Med", "High", "Xhigh", "Max"]
        );
    }
    assert_eq!(
        EffortsSelection::Claude.get_efforts("claude-sonnet-4-6"),
        ["Low", "Med", "High", "Max"]
    );
    assert!(EffortsSelection::Other.get_efforts("custom").is_empty());
}
