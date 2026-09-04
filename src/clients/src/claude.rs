use crate::claude_config::{ClaudeAuthConfig, ClaudeConfig, ClaudeEffort};
use crate::llm;
use crate::llm::{ClientResponse, LLmClientTrait};
use anyhow::{Error, anyhow};
use async_stream::try_stream;
use futures::{Stream, StreamExt};
use reqwest::{Client, header};
use reqwest_middleware::{ClientBuilder, ClientWithMiddleware};
use reqwest_retry::RetryTransientMiddleware;
use reqwest_retry::policies::ExponentialBackoff;
use reqwest_tracing::TracingMiddleware;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::Duration;
use thiserror::Error;

pub const MAX_TOKENS: u32 = 64000;
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
            content: vec![(ContentBlock::MessageBlock { text: message })],
        }
    }

    pub fn new_assistant(message: String) -> Self {
        Message {
            role: Role::Assistant,
            content: vec![(ContentBlock::MessageBlock { text: message })],
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<ClaudeEffort>,
}

#[derive(Debug, Serialize)]
struct ChatRequestStream {
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<ClaudeEffort>,
    pub stream: bool,
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
pub struct ToolBlock {
    #[serde(rename = "type")]
    pub content_type: String,
}
#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(tag = "type")]
pub enum ContentBlock {
    #[serde(rename = "text")]
    MessageBlock { text: String },
    #[serde(rename = "thinking")]
    ThinkingBlock { thinking: String, signature: String },
    #[serde(rename = "tool_use")]
    ToolBlock {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    #[serde(rename = "tool_result")]
    ToolResult {
        tool_use_id: String,
        content: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        is_error: Option<bool>,
    },
    #[serde(rename = "server_tool_use")]
    ServerToolUse {
        id: String,
        name: String,
        #[serde(default)]
        input: serde_json::Value,
    },
    #[serde(rename = "web_search_tool_result")]
    WebSearchToolResult {
        tool_use_id: String,
        #[serde(default)]
        content: serde_json::Value,
    },
}

// Streaming types
#[derive(Debug, Deserialize, Clone)]
#[serde(tag = "type")]
pub enum StreamEvent {
    #[serde(rename = "message_start")]
    MessageStart { message: StreamMessage },
    #[serde(rename = "content_block_start")]
    ContentBlockStart {
        index: usize,
        content_block: ContentBlockInfo,
    },
    #[serde(rename = "content_block_delta")]
    ContentBlockDelta { index: usize, delta: Delta },
    #[serde(rename = "content_block_stop")]
    ContentBlockStop { index: usize },
    #[serde(rename = "message_delta")]
    MessageDelta {
        delta: MessageDeltaContent,
        usage: UsageDelta,
    },
    #[serde(rename = "message_stop")]
    MessageStop,
    #[serde(rename = "ping")]
    Ping,
    #[serde(rename = "error")]
    Error { error: ApiErrorDetail },
}

#[derive(Debug, Deserialize, Clone, Default)]
pub struct StreamUsage {
    #[serde(default)]
    pub input_tokens: u32,
    #[serde(default)]
    pub cache_creation_input_tokens: u32,
    #[serde(default)]
    pub cache_read_input_tokens: u32,
    #[serde(default)]
    pub output_tokens: u32,
}

#[derive(Debug, Deserialize, Clone)]
pub struct StreamMessage {
    pub id: String,
    pub model: String,
    pub role: Role,
    pub usage: StreamUsage,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(tag = "type")]
pub enum ContentBlockInfo {
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    #[serde(rename = "thinking")]
    Thinking { thinking: String },
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "server_tool_use")]
    ServerToolUse {
        id: String,
        name: String,
        #[serde(default)]
        input: serde_json::Value,
    },
    #[serde(rename = "web_search_tool_result")]
    WebSearchToolResult {
        tool_use_id: String,
        #[serde(default)]
        content: serde_json::Value,
    },
}

#[derive(Debug, Deserialize, Clone)]
#[serde(tag = "type")]
pub enum Delta {
    #[serde(rename = "text_delta")]
    TextDelta { text: String },
    #[serde(rename = "thinking_delta")]
    ThinkingDelta { thinking: String },
    #[serde(rename = "input_json_delta")]
    InputJsonDelta { partial_json: String },
    #[serde(rename = "signature_delta")]
    SignatureDelta { signature: String },
}

#[derive(Debug, Deserialize, Clone)]
pub struct MessageDeltaContent {
    pub stop_reason: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct UsageDelta {
    pub output_tokens: u32,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ApiErrorDetail {
    #[serde(rename = "type")]
    pub error_type: String,
    pub message: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(untagged)]
pub enum Tool {
    Client {
        name: String,
        description: String,
        input_schema: ToolSchemaDTO,
    },
    Server {
        #[serde(rename = "type")]
        tool_type: String,
        name: String,
        max_uses: u32,
    },
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
#[serde(untagged)]
pub enum ToolProperty {
    Value {
        #[serde(skip)]
        name: String,
        #[serde(rename = "type")]
        prop_type: String,
        description: String,
    },
    Object {
        #[serde(skip)]
        name: String,
        #[serde(rename = "type")]
        prop_type: String,
        description: String,
        properties: HashMap<String, ToolProperty>,
    },
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct CacheControl {
    pub(crate) cache_type: String,
    pub(crate) ttl: String,
}

#[derive(Debug, Clone)]
pub struct ClaudeClient {
    client: ClientWithMiddleware,
    base_url: String,
    config: ClaudeConfig,
}

pub struct ClientRequest {
    pub messages: Vec<Message>,
    pub thinking: bool,
    pub system: Option<String>,
    pub model: Option<String>,
    // this shit needs to be turned ON
    pub(crate) cache_control: CacheControl,
    pub tools: Vec<Tool>,
    pub effort: Option<ClaudeEffort>,
}

impl ClaudeClient {
    const BASE_URL: &'static str = "https://api.anthropic.com/v1";
    const API_VERSION: &'static str = "2023-06-01";
    pub fn new(config: ClaudeConfig) -> ClaudeResult<Self> {
        let mut headers = header::HeaderMap::new();
        headers.insert(
            "x-api-key",
            header::HeaderValue::from_str(
                &match &config.auth {
                    ClaudeAuthConfig::APIKey(key) => key,
                }
                .api_key,
            )
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

        let retry_policy = ExponentialBackoff::builder().build_with_max_retries(3);
        let inner_client = Client::builder()
            .connect_timeout(Duration::from_secs(60))
            .read_timeout(Duration::from_secs(300))
            .default_headers(headers)
            .build()?;

        let client = ClientBuilder::new(inner_client)
            .with(TracingMiddleware::default())
            .with(RetryTransientMiddleware::new_with_policy(retry_policy))
            .build();

        Ok(Self {
            client,
            config,
            base_url: Self::BASE_URL.to_string(),
        })
    }
    pub async fn chat(&self, req: ClientRequest) -> ClaudeResult<ChatResponse> {
        let inner_req = ChatRequest {
            model: req.model.unwrap_or(self.config.model.clone()),
            max_tokens: MAX_TOKENS,
            messages: req.messages,
            system: req.system,
            temperature: None,
            thinking: match req.thinking {
                true => Some(Thinking {
                    thinking_type: ThinkingType::Enabled,
                    budget_tokens: 1024,
                }),
                false => None,
            },
            tools: req.tools,
            effort: req.effort,
        };
        self.send_request(inner_req).await
    }

    async fn send_request(&self, request: ChatRequest) -> ClaudeResult<ChatResponse> {
        let url = format!("{}/messages", self.base_url);

        let response = self
            .client
            .post(&url)
            .json(&request)
            .send()
            .await
            .map_err(|e| ClaudeError::ApiError {
                message: e.to_string(),
            })?;
        let chat_response: ChatResponse = response.json().await?;
        Ok(chat_response)
    }

    pub async fn chat_stream_claude(
        &self,
        req: ClientRequest,
    ) -> Result<impl Stream<Item = ClaudeResult<StreamEvent>> + Send + 'static, anyhow::Error> {
        let url = format!("{}/messages", self.base_url);
        let request = ChatRequestStream {
            model: req.model.unwrap_or_else(|| self.config.model.clone()),
            max_tokens: MAX_TOKENS,
            messages: req.messages,
            system: req.system,
            temperature: None,
            thinking: if req.thinking {
                Some(Thinking {
                    thinking_type: ThinkingType::Enabled,
                    budget_tokens: 1024,
                })
            } else {
                None
            },
            tools: req.tools,
            stream: true,
            effort: req.effort,
        };

        let initial = self.client.post(&url).json(&request).send().await?;

        if !initial.status().is_success() {
            let status = initial.status();
            let body = initial.text().await.unwrap_or_default();
            return Err(anyhow!("API error {status}: {body}"));
        }

        let mut byte_stream = initial.bytes_stream();
        let mut buffer = String::new();

        let stream = try_stream! {
            while let Some(chunk) = byte_stream.next().await {
                let chunk = chunk?;
                buffer.push_str(&String::from_utf8_lossy(&chunk));

                while let Some(stream_event) = extract_sse_event(&mut buffer)?{
                    yield stream_event;
                }
            }
        };
        Ok(stream)
    }
}

fn extract_sse_event(buffer: &mut String) -> ClaudeResult<Option<StreamEvent>> {
    let delimiter_pos = match buffer.find("\n\n") {
        Some(pos) => pos,
        None => return Ok(None),
    };

    let event_text = buffer[..delimiter_pos].to_string();
    buffer.drain(..=delimiter_pos + 1);

    let data_line = event_text.lines().fold(String::new(), |mut acc, line| {
        let line = line.trim();
        if let Some(data) = line.strip_prefix("data: ") {
            acc.push_str(data);
        }
        acc
    });

    let data = match data_line.is_empty() {
        true => return Ok(None),
        false => data_line,
    };

    if data == "[DONE]" {
        return Ok(None);
    }

    match serde_json::from_str::<StreamEvent>(data.as_str()) {
        Ok(event) => Ok(Some(event)),
        Err(e) => Err(ClaudeError::Serialization(e)),
    }
}

impl LLmClientTrait for ClaudeClient {
    async fn chat_stream(
        &self,
        req: llm::ClientRequest,
    ) -> Result<impl Stream<Item = anyhow::Result<llm::StreamEvent>> + Send + 'static, Error> {
        let mut claude_req: ClientRequest = req.try_into()?;
        claude_req.effort = Some(self.config.effort.clone());
        let stream = self.chat_stream_claude(claude_req).await?;
        Ok(stream.map(|res| match res {
            Ok(event) => Ok(event.into()),
            Err(e) => Err(anyhow!(e)),
        }))
    }

    async fn send_request(&self, request: llm::ClientRequest) -> anyhow::Result<ClientResponse> {
        let claude_req: ClientRequest = request.try_into()?;
        match self.chat(claude_req).await {
            Ok(res) => Ok(res.into()),
            Err(e) => Err(anyhow!(e)),
        }
    }
}
