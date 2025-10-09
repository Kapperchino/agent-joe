use ra_ap_hir::sym::bool;
use reqwest::{Client, StatusCode, header};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::Duration;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ClaudeError {
    #[error("HTTP request failed: {0}")]
    RequestFailed(#[from] reqwest::Error),
    #[error("API error: {message}")]
    ApiError { message: String },
    #[error("Invalid configuration: {0}")]
    Config(String),
    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
}
pub type ClaudeResult<T> = Result<T, ClaudeError>;
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: Vec<ContentBlock>,
}

impl Message {
    pub fn new(message: String) -> Self {
        Message {
            role: Role::User,
            content: vec![
                (ContentBlock::MessageBlock(MessageBlock {
                    content_type: "text".to_string(),
                    text: message,
                })),
            ],
        }
    }
}
#[derive(Debug, Serialize)]
pub struct ChatRequest {
    pub model: String,
    pub max_tokens: u32,
    pub messages: Vec<Message>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<Thinking>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<Tool>,
}
#[derive(Debug, Serialize)]
pub struct Thinking {
    #[serde(rename = "type")]
    pub thinking_type: ThinkingType,
    pub budget_tokens: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThinkingType {
    Enabled,
}
#[derive(Debug, Deserialize)]
pub struct Usage {
    pub input_tokens: u32,
    pub cache_creation_input_tokens: u32,
    pub cache_read_input_tokens: u32,
    pub output_tokens: u32,
    pub service_tier: String,
    pub cache_creation: CacheCreation,
}

#[derive(Debug, Deserialize)]
pub struct CacheCreation {
    pub ephemeral_5m_input_tokens: u32,
    pub ephemeral_1h_input_tokens: u32,
}
#[derive(Debug, Deserialize)]
pub struct ChatResponse {
    pub id: String,
    pub model: String,
    #[serde(rename = "type")]
    pub res_type: String,
    pub role: Role,
    pub content: Vec<ContentBlock>,
    pub stop_reason: Option<String>,
    pub stop_sequence: Option<String>,
    pub usage: Usage,
}
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct MessageBlock {
    #[serde(rename = "type")]
    pub content_type: String,
    pub text: String,
}
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ThinkingBlock {
    #[serde(rename = "type")]
    pub content_type: String,
    pub thinking: String,
    pub signature: String,
}
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ToolBlock {
    #[serde(rename = "type")]
    pub content_type: String,
    pub id: String,
    pub name: String,
    pub input: HashMap<String, String>,
}
#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(untagged)]
pub enum ContentBlock {
    MessageBlock(MessageBlock),
    ThinkingBlock(ThinkingBlock),
    ToolBlock(ToolBlock),
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Tool {
    pub name: String,
    pub description: String,
    pub input_schema: ToolSchemaDTO,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ToolSchemaDTO {
    #[serde(skip)]
    pub name: String,
    #[serde(rename = "type")]
    pub tool_type: String,
    pub properties: HashMap<String, ToolProperty>,
    pub required: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ToolSchema {
    pub name: String,
    pub tool_type: String,
    pub properties: Vec<ToolProperty>,
    pub required: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ToolProperty {
    #[serde(skip)]
    pub name: String,
    #[serde(rename = "type")]
    pub prop_type: String,
    pub description: String,
}

#[derive(Debug, Clone)]
pub struct ClaudeConfig {
    pub api_key: String,
    pub model: String,
    pub max_tokens: u32,
    pub temperature: Option<f32>,
    pub timeout: Duration,
    pub tools: Vec<Tool>,
}

impl Default for ClaudeConfig {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            model: "claude-sonnet-4-5-20250929".to_string(),
            max_tokens: 30000,
            temperature: Some(1.0),
            timeout: Duration::from_secs(60),
            tools: vec![],
        }
    }
}

#[derive(Debug)]
pub struct ClaudeClient {
    client: Client,
    config: ClaudeConfig,
    base_url: String,
}

pub struct ClientRequest {
    messages: Vec<Message>,
    thinking: bool,
    system: Option<String>,
    model: Option<String>,
    tools: Vec<Tool>,
}

impl ClientRequest {
    pub(crate) fn new(messages: Vec<Message>) -> ClientRequest {
        ClientRequest {
            messages,
            thinking: false,
            system: None,
            model: None,
            tools: vec![],
        }
    }

    pub fn with_thinking(self) -> ClientRequest {
        ClientRequest {
            messages: self.messages,
            thinking: true,
            system: self.system,
            model: self.model,
            tools: self.tools,
        }
    }

    pub fn with_model(self, model: String) -> ClientRequest {
        ClientRequest {
            messages: self.messages,
            thinking: self.thinking,
            system: self.system,
            model: Some(model),
            tools: self.tools,
        }
    }

    pub fn with_tools(self, tools: Vec<Tool>) -> ClientRequest {
        ClientRequest {
            messages: self.messages,
            thinking: self.thinking,
            system: self.system,
            model: self.model,
            tools,
        }
    }
}

impl ClaudeClient {
    const BASE_URL: &'static str = "https://api.anthropic.com/v1";
    const API_VERSION: &'static str = "2023-06-01";
    pub fn new(config: ClaudeConfig) -> ClaudeResult<Self> {
        if config.api_key.is_empty() {
            return Err(ClaudeError::Config("API key is required".to_string()));
        }

        let mut headers = header::HeaderMap::new();
        headers.insert(
            "x-api-key",
            header::HeaderValue::from_str(&config.api_key)
                .map_err(|_| ClaudeError::Config("Invalid API key format".to_string()))?,
        );
        headers.insert(
            "anthropic-version",
            header::HeaderValue::from_static(Self::API_VERSION),
        );
        headers.insert(
            header::CONTENT_TYPE,
            header::HeaderValue::from_static("application/json"),
        );

        let client = Client::builder()
            .timeout(config.timeout)
            .default_headers(headers)
            .build()?;

        Ok(Self {
            client,
            config,
            base_url: Self::BASE_URL.to_string(),
        })
    }
    pub async fn chat(&self, req: ClientRequest) -> ClaudeResult<ChatResponse> {
        let inner_req = ChatRequest {
            model: req.model.unwrap_or(self.config.model.clone()),
            max_tokens: self.config.max_tokens,
            messages: req.messages,
            system: req.system,
            temperature: self.config.temperature,
            thinking: match req.thinking {
                true => Some(Thinking {
                    thinking_type: ThinkingType::Enabled,
                    budget_tokens: 1024,
                }),
                false => None,
            },
            tools: req.tools,
        };
        self.send_request(inner_req).await
    }

    async fn send_request(&self, request: ChatRequest) -> ClaudeResult<ChatResponse> {
        let url = format!("{}/messages", self.base_url);

        let response = self.client.post(&url).json(&request).send().await?;
        let chat_response: ChatResponse = response.json().await?;
        Ok(chat_response)
    }
}
