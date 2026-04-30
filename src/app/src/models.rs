use clients::{ClaudeEffort, OpenAIEffort};
use strum::VariantNames;
use strum_macros::{EnumString, VariantNames};

#[derive(Debug, PartialEq, Clone)]
pub enum EffortsSelection {
    OpenAI,
    Claude,
    Other,
}
#[derive(Debug, PartialEq, Clone)]
pub enum ModelSelections {
    OpenAI,
    Claude,
    Other,
}

impl ModelSelections {
    pub fn get_models(&self) -> Vec<String> {
        match self {
            ModelSelections::OpenAI => OpenAIModels::VARIANTS
                .into_iter()
                .map(|x| x.to_string())
                .collect(),
            ModelSelections::Claude => ClaudeModels::VARIANTS
                .into_iter()
                .map(|x| x.to_string())
                .collect(),
            ModelSelections::Other => {
                vec![]
            }
        }
    }
}

impl EffortsSelection {
    pub fn get_efforts(&self) -> Vec<String> {
        match self {
            EffortsSelection::OpenAI => OpenAIEffort::VARIANTS
                .into_iter()
                .map(|x| x.to_string())
                .collect(),
            EffortsSelection::Claude => ClaudeEffort::VARIANTS
                .into_iter()
                .map(|x| x.to_string())
                .collect(),
            EffortsSelection::Other => {
                vec![]
            }
        }
    }
}
#[derive(Debug, PartialEq, EnumString, VariantNames, Clone)]
pub enum OpenAIModels {
    #[strum(serialize = "gpt-5.5")]
    GPT5_5,
    #[strum(serialize = "gpt-5.4")]
    GPT5_4,
    #[strum(serialize = "gpt-5.3-codex")]
    GPT5_3_Codex,
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
