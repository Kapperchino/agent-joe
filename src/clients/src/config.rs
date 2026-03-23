use figment::Figment;
use figment::providers::{Env, Format, Toml};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use strum_macros::EnumMessage;

pub const CONFIG_PATH: &str = "~/.turbo-code/config.toml";

#[derive(Debug, Serialize, Deserialize)]
pub enum Config {
    Claude(ClaudeConfig),
    OpenAI(OpenAIConfig),
}

impl Config {
    pub fn new() -> anyhow::Result<Config> {
        let figment: Config = Figment::new().merge(Toml::file(CONFIG_PATH)).extract()?;
        Ok(figment)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaudeConfig {
    pub auth: ClaudeAuthConfig,
    pub model: String,
    pub effort: ClaudeEffort,
}
#[derive(Debug, Serialize, Deserialize)]
struct OpenAIConfig {
    pub auth: OpenAIAuthConfig,
    pub model: String,
    pub effort: OpenAIEffort,
}

#[derive(PartialEq, Eq, Debug, Clone, EnumMessage, Serialize, Deserialize)]
pub enum ClaudeEffort {
    #[strum(message = "low")]
    #[serde(rename = "low")]
    Low,
    #[strum(message = "medium")]
    #[serde(rename = "medium")]
    Med,
    #[strum(message = "high")]
    #[serde(rename = "high")]
    High,
    #[strum(message = "max")]
    #[serde(rename = "max")]
    Max,
}
#[derive(PartialEq, Eq, Debug, EnumMessage, Serialize, Deserialize)]
enum OpenAIEffort {
    #[strum(message = "low")]
    #[serde(rename = "low")]
    Low,
    #[strum(message = "medium")]
    #[serde(rename = "medium")]
    Med,
    #[strum(message = "high")]
    #[serde(rename = "high")]
    High,
    #[strum(message = "xhigh")]
    #[serde(rename = "xhigh")]
    XHigh,
}

// we could add auth login later, could get ppl banned
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ClaudeAuthConfig {
    APIKey(ClaudeKeyConfig),
}

#[derive(Debug, Serialize, Deserialize)]
enum OpenAIAuthConfig {
    APIKey(OpenAIKeyConfig),
    Codex(OpenAICodexConfig),
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ClaudeKeyConfig {
    pub api_key: String,
}
#[derive(Debug, Deserialize, Serialize, Clone)]
struct OpenAIKeyConfig {
    api_key: String,
    url: String, // could point to other providers
}
#[derive(Debug, Deserialize, Serialize, Clone)]
struct OpenAICodexConfig {
    id_token: String,
    access_token: String,
    refresh_token: String,
    account_id: String,
    last_refresh: Duration,
}
