use serde::{Deserialize, Serialize};
use strum_macros::{EnumMessage, EnumString};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaudeConfig {
    pub auth: ClaudeAuthConfig,
    pub model: String,
    pub effort: ClaudeEffort,
}

#[derive(PartialEq, Eq, Debug, Clone, EnumString, EnumMessage, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClaudeEffort {
    #[strum(message = "low")]
    Low,
    #[strum(message = "medium")]
    Med,
    #[strum(message = "high")]
    High,
    #[strum(message = "max")]
    Max,
}

// we could add auth login later, could get ppl banned
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ClaudeAuthConfig {
    APIKey(ClaudeKeyConfig),
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ClaudeKeyConfig {
    pub api_key: String,
}
