pub use clients::models::{ClaudeModels, OpenAIModels};
use clients::{ClaudeEffort, OpenAIEffort};
use strum::VariantNames;

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
    pub fn get_efforts(&self, model: &str) -> Vec<String> {
        match self {
            EffortsSelection::OpenAI => OpenAIEffort::supported_for_model(model)
                .iter()
                .map(|effort| effort.as_ref().to_string())
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
