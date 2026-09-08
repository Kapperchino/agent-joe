use strum_macros::{EnumString, VariantNames};

pub const FALLBACK_CONTEXT_WINDOW: usize = 128_000;

#[derive(Debug, PartialEq, EnumString, VariantNames, Clone)]
pub enum OpenAIModels {
    #[strum(serialize = "gpt-6-astra")]
    GPT6_ASTRA,
    #[strum(serialize = "gpt-5.6-sol")]
    GPT5_6_SOL,
    #[strum(serialize = "gpt-5.6-terra")]
    GPT5_6_TERRA,
    #[strum(serialize = "gpt-5.6-luna")]
    GPT5_6_LUNA,
    #[strum(serialize = "gpt-5.5")]
    GPT5_5,
    #[strum(serialize = "gpt-5.4")]
    GPT5_4,
}

impl OpenAIModels {
    pub fn context_window(&self) -> usize {
        match self {
            Self::GPT6_ASTRA => 1_050_000,
            Self::GPT5_6_SOL => 1_050_000,
            Self::GPT5_6_TERRA => 1_050_000,
            Self::GPT5_6_LUNA => 1_050_000,
            Self::GPT5_5 => 1_050_000,
            Self::GPT5_4 => 1_050_000,
        }
    }

    pub fn codex_context_window(&self) -> usize {
        match self {
            Self::GPT6_ASTRA => 272_000,
            Self::GPT5_6_SOL => 272_000,
            Self::GPT5_6_TERRA => 272_000,
            Self::GPT5_6_LUNA => 272_000,
            Self::GPT5_5 => 272_000,
            Self::GPT5_4 => 272_000,
        }
    }
}

#[derive(Debug, PartialEq, EnumString, VariantNames, Clone)]
pub enum ClaudeModels {
    #[strum(serialize = "claude-opus-4-7")]
    Opus4_7,
    #[strum(serialize = "claude-sonnet-4-6")]
    Sonnet4_6,
    #[strum(serialize = "claude-haiku-4-5")]
    Haiku4_5,
}

impl ClaudeModels {
    pub fn context_window(&self) -> usize {
        match self {
            Self::Opus4_7 => 1_000_000,
            Self::Sonnet4_6 => 1_000_000,
            Self::Haiku4_5 => 200_000,
        }
    }
}

pub fn context_window(model: &str) -> usize {
    let model = model_name(model);
    model
        .parse::<OpenAIModels>()
        .map(|model| model.context_window())
        .or_else(|_| {
            model
                .parse::<ClaudeModels>()
                .map(|model| model.context_window())
        })
        .unwrap_or(match model {
            "gpt-5" => 400_000,
            "claude-sonnet-4-20250514" => 200_000,
            _ => FALLBACK_CONTEXT_WINDOW,
        })
}

pub(crate) fn codex_context_window(model: &str) -> usize {
    model_name(model)
        .parse::<OpenAIModels>()
        .map(|model| model.codex_context_window())
        .unwrap_or(FALLBACK_CONTEXT_WINDOW)
}

fn model_name(model: &str) -> &str {
    let model = model
        .strip_prefix("openai/")
        .or_else(|| model.strip_prefix("anthropic/"))
        .unwrap_or(model);
    match model {
        "gpt-5.6" => "gpt-5.6-sol",
        "claude-haiku-4-5-20251001" => "claude-haiku-4-5",
        "gpt-5.4-2026-03-05" => "gpt-5.4",
        _ => model,
    }
}

#[cfg(test)]
mod tests {
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
}
