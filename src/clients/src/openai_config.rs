use crate::openai::{ReasoningConfig, ReasoningSummary};
use serde::{Deserialize, Serialize};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use anyhow::Context;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use strum_macros::EnumMessage;

const CHATGPT_CODEX_BASE_URL: &str = "https://chatgpt.com/backend-api/codex";
const CHATGPT_BASE_URL: &str = "https://api.openai.com/v1";
const OPENROUTER_BASE_URL: &str = "https://openrouter.ai/api/v1";
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAIConfig {
    pub auth: OpenAIAuthConfig,
    pub model: String,
    pub effort: OpenAIEffort,
}

impl OpenAIConfig {
    pub fn get_url(&self) -> String {
        match &self.auth {
            OpenAIAuthConfig::APIKey(conf) => {
                conf.url.clone().unwrap_or(CHATGPT_BASE_URL.to_string())
            }
            OpenAIAuthConfig::Codex(_) => CHATGPT_CODEX_BASE_URL.to_string(),
            OpenAIAuthConfig::Local(conf) => conf.url.clone(),
            OpenAIAuthConfig::OpenRouter(conf) => conf
                .url
                .clone()
                .unwrap_or(OPENROUTER_BASE_URL.to_string()),
        }
    }

    pub fn get_reasoning(&self) -> ReasoningConfig {
        ReasoningConfig {
            effort: self.effort.clone(),
            summary: ReasoningSummary::Auto,
        }
    }
}

#[derive(PartialEq, Eq, Debug, Clone, EnumMessage, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpenAIEffort {
    #[strum(message = "low")]
    Low,
    #[strum(message = "medium")]
    Medium,
    #[strum(message = "high")]
    High,
    #[strum(message = "xhigh")]
    Xhigh,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum OpenAIAuthConfig {
    APIKey(OpenAIKeyConfig),
    Codex(OpenAICodexConfig),
    Local(LocalOpenAIConfig),
    OpenRouter(OpenRouterConfig),
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct OpenAIKeyConfig {
    pub api_key: String,
    pub url: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct LocalOpenAIConfig {
    pub api_key: Option<String>,
    pub url: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct OpenRouterConfig {
    pub api_key: String,
    pub url: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct OpenAICodexConfig {
    pub id_token: String,
    pub access_token: String,
    pub refresh_token: String,
    pub account_id: String,
    pub last_refresh: Duration,
    #[serde(default)]
    pub expires_at_ms: u64,
}
