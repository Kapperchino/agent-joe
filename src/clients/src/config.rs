use serde::{Deserialize, Serialize};
use std::time::Duration;
use strum_macros::EnumMessage;

#[derive(Debug, Serialize, Deserialize)]
enum Config {
    Claude(ClaudeConfig),
    OpenAI(OpenAIConfig),
}

#[derive(Debug, Serialize, Deserialize)]
struct ClaudeConfig {
    auth: ClaudeAuthConfig,
    model: String,
    effort: ClaudeEffort,
}
#[derive(Debug, Serialize, Deserialize)]
struct OpenAIConfig {
    auth: OpenAIAuthConfig,
    model: String,
    effort: OpenAIEffort,
}

#[derive(PartialEq, Eq, Debug, EnumMessage, Serialize, Deserialize)]
enum ClaudeEffort {
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
#[derive(Debug, Serialize, Deserialize)]
enum ClaudeAuthConfig {
    APIKey(ClaudeKeyConfig),
}

#[derive(Debug, Serialize, Deserialize)]
enum OpenAIAuthConfig {
    APIKey(OpenAIKeyConfig),
    Codex(OpenAICodexConfig),
}

#[derive(Debug, Deserialize, Serialize, Clone)]
struct ClaudeKeyConfig {
    api_key: String,
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
