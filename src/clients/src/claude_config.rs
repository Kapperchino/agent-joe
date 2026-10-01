use serde::{Deserialize, Serialize};
use strum_macros::{AsRefStr, EnumMessage, EnumString, VariantNames};

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct ClaudeConfig {
    pub auth: ClaudeAuthConfig,
    pub model: String,
    pub effort: ClaudeEffort,
}

#[derive(
    PartialEq,
    Eq,
    Debug,
    Clone,
    EnumString,
    EnumMessage,
    VariantNames,
    Serialize,
    Deserialize,
    AsRefStr,
)]
#[serde(rename_all = "snake_case")]
pub enum ClaudeEffort {
    #[strum(message = "low")]
    Low,
    #[strum(message = "medium")]
    #[serde(rename = "medium", alias = "med")]
    Med,
    #[strum(message = "high")]
    High,
    #[strum(message = "xhigh")]
    Xhigh,
    #[strum(message = "max")]
    Max,
}

impl ClaudeEffort {
    pub fn supported_for_model(model: &str) -> &'static [Self] {
        match crate::models::model_name(model) {
            "claude-opus-5-5" | "claude-fable-5-1" | "claude-sonnet-5-5" | "claude-opus-4-7" => {
                &[Self::Low, Self::Med, Self::High, Self::Xhigh, Self::Max]
            }
            _ => &[Self::Low, Self::Med, Self::High, Self::Max],
        }
    }
}

// we could add auth login later, could get ppl banned
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum ClaudeAuthConfig {
    APIKey(ClaudeKeyConfig),
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
pub struct ClaudeKeyConfig {
    pub api_key: String,
}
