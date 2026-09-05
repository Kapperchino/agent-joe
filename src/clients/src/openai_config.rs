use crate::openai::{ReasoningConfig, ReasoningSummary, ResponseInclude};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use strum_macros::{AsRefStr, EnumMessage, EnumString, VariantNames};

const CHATGPT_CODEX_BASE_URL: &str = "https://chatgpt.com/backend-api/codex";
const CHATGPT_BASE_URL: &str = "https://api.openai.com/v1";
const OPENROUTER_BASE_URL: &str = "https://openrouter.ai/api/v1";
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct OpenAIConfig {
    pub auth: OpenAIAuthConfig,
    pub model: String,
    pub effort: OpenAIEffort,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_encrypted_reasoning: Option<bool>,
}

impl OpenAIConfig {
    pub fn reasoning_include(&self) -> Vec<ResponseInclude> {
        if self
            .request_encrypted_reasoning
            .unwrap_or_else(|| self.get_url().trim_end_matches('/') == CHATGPT_BASE_URL)
        {
            vec![ResponseInclude::EncryptedReasoning]
        } else {
            vec![]
        }
    }

    pub fn get_url(&self) -> String {
        match &self.auth {
            OpenAIAuthConfig::APIKey(conf) => {
                conf.url.clone().unwrap_or(CHATGPT_BASE_URL.to_string())
            }
            OpenAIAuthConfig::Codex(_) => CHATGPT_CODEX_BASE_URL.to_string(),
            OpenAIAuthConfig::Local(conf) => conf.url.clone(),
            OpenAIAuthConfig::OpenRouter(conf) => {
                conf.url.clone().unwrap_or(OPENROUTER_BASE_URL.to_string())
            }
        }
    }

    pub fn get_reasoning(&self) -> ReasoningConfig {
        ReasoningConfig {
            effort: self.effort.clone(),
            summary: ReasoningSummary::Auto,
        }
    }
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
pub enum OpenAIEffort {
    #[strum(message = "none")]
    None,
    #[strum(message = "low")]
    Low,
    #[strum(message = "medium")]
    Medium,
    #[strum(message = "high")]
    High,
    #[strum(message = "xhigh")]
    Xhigh,
    #[strum(message = "max")]
    Max,
}

impl OpenAIEffort {
    pub fn supported_for_model(model: &str) -> &'static [Self] {
        match model {
            "gpt-6-astra" => &[Self::Low, Self::Medium, Self::High, Self::Xhigh, Self::Max],
            "gpt-5.6" | "gpt-5.6-sol" | "gpt-5.6-terra" | "gpt-5.6-luna" => &[
                Self::None,
                Self::Low,
                Self::Medium,
                Self::High,
                Self::Xhigh,
                Self::Max,
            ],
            _ => &[Self::None, Self::Low, Self::Medium, Self::High, Self::Xhigh],
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub enum OpenAIAuthConfig {
    APIKey(OpenAIKeyConfig),
    Codex(OpenAICodexConfig),
    Local(LocalOpenAIConfig),
    OpenRouter(OpenRouterConfig),
}

#[derive(Debug, Eq, PartialEq, Deserialize, Serialize, Clone)]
pub struct OpenAIKeyConfig {
    pub api_key: String,
    pub url: Option<String>,
}

#[derive(Debug, Eq, PartialEq, Deserialize, Serialize, Clone)]
pub struct LocalOpenAIConfig {
    pub api_key: Option<String>,
    pub url: String,
}

#[derive(Debug, Eq, PartialEq, Deserialize, Serialize, Clone)]
pub struct OpenRouterConfig {
    pub api_key: String,
    pub url: Option<String>,
}

#[derive(Debug, Eq, PartialEq, Deserialize, Serialize, Clone)]
pub struct OpenAICodexConfig {
    pub id_token: String,
    pub access_token: String,
    pub refresh_token: String,
    pub account_id: String,
    pub last_refresh: Duration,
    #[serde(default)]
    pub expires_at_ms: u64,
}
