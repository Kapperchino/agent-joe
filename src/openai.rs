use anyhow::anyhow;
use async_stream::try_stream;
use futures::{Stream, StreamExt};
use reqwest::{header, Client};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::Duration;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum OpenAIError {
    #[error("HTTP request failed: {0}")]
    RequestFailed(#[from] reqwest::Error),
    #[error("API error: {message}")]
    ApiError { message: String },
    #[error("Invalid configuration: {0}")]
    Config(String),
    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
}

pub type OpenAIResult<T> = Result<T, OpenAIError>;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

impl Message {
    pub fn user(content: String) -> Self {
        Message {
            role: Role::User,
            content: Some(content),
            tool_calls: None,
            tool_call_id: None,
        }
    }

    pub fn assistant(content: String) -> Self {
        Message {
            role: Role::Assistant,
            content: Some(content),
            tool_calls: None,
            tool_call_id: None,
        }
    }

    pub fn assistant_tool_calls(tool_calls: Vec<ToolCall>) -> Self {
        Message {
            role: Role::Assistant,
            content: None,
            tool_calls: Some(tool_calls),
            tool_call_id: None,
        }
    }

    pub fn tool_result(tool_call_id: String, content: String) -> Self {
        Message {
            role: Role::Tool,
            content: Some(content),
            tool_calls: None,
            tool_call_id: Some(tool_call_id),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub call_type: String,
    pub function: FunctionCall,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionCall {
    pub name: String,
    pub arguments: String,
}

// --- Tool definition types ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tool {
    #[serde(rename = "type")]
    pub tool_type: String,
    pub function: FunctionDef,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionDef {
    pub name: String,
    pub description: String,
    pub parameters: FunctionParameters,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionParameters {
    #[serde(rename = "type")]
    pub param_type: String,
    pub properties: HashMap<String, ToolProperty>,
    pub required: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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

impl From<crate::claude::ToolProperty> for ToolProperty {
    fn from(cp: crate::claude::ToolProperty) -> Self {
        match cp {
            crate::claude::ToolProperty::Value {
                name,
                prop_type,
                description,
            } => ToolProperty::Value {
                name,
                prop_type,
                description,
            },
            crate::claude::ToolProperty::Object {
                name,
                prop_type,
                description,
                properties,
            } => ToolProperty::Object {
                name,
                prop_type,
                description,
                properties: properties.into_iter().map(|(k, v)| (k, v.into())).collect(),
            },
        }
    }
}

/// Convert a claude::Tool into an OpenAI Tool definition.
impl From<crate::claude::Tool> for Tool {
    fn from(ct: crate::claude::Tool) -> Self {
        Tool {
            tool_type: "function".to_string(),
            function: FunctionDef {
                name: ct.name,
                description: ct.description,
                parameters: FunctionParameters {
                    param_type: "object".to_string(),
                    properties: ct
                        .input_schema
                        .properties
                        .into_iter()
                        .map(|(k, v)| (k, v.into()))
                        .collect(),
                    required: ct.input_schema.required,
                },
            },
        }
    }
}

// --- Request types ---

#[derive(Debug, Serialize)]
struct ChatCompletionRequest {
    pub model: String,
    pub messages: Vec<Message>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<Tool>,
    pub stream: bool,
}

// --- Response types (non-streaming) ---

#[derive(Debug, Deserialize)]
pub struct ChatCompletionResponse {
    pub id: String,
    pub object: String,
    pub model: String,
    pub choices: Vec<Choice>,
    pub usage: Usage,
}

#[derive(Debug, Deserialize)]
pub struct Choice {
    pub index: usize,
    pub message: ChoiceMessage,
    pub finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ChoiceMessage {
    pub role: Role,
    pub content: Option<String>,
    pub tool_calls: Option<Vec<ToolCall>>,
}

#[derive(Debug, Deserialize, Clone, Default)]
pub struct Usage {
    #[serde(default)]
    pub prompt_tokens: u32,
    #[serde(default)]
    pub completion_tokens: u32,
    #[serde(default)]
    pub total_tokens: u32,
}

// --- Streaming types ---

#[derive(Debug, Deserialize, Clone)]
pub struct StreamChunk {
    pub id: String,
    pub object: String,
    pub model: String,
    pub choices: Vec<StreamChoice>,
    #[serde(default)]
    pub usage: Option<Usage>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct StreamChoice {
    pub index: usize,
    pub delta: StreamDelta,
    pub finish_reason: Option<String>,
}

#[derive(Debug, Deserialize, Clone, Default)]
pub struct StreamDelta {
    #[serde(default)]
    pub role: Option<Role>,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub tool_calls: Option<Vec<StreamToolCall>>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct StreamToolCall {
    pub index: usize,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub function: Option<StreamFunctionCall>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct StreamFunctionCall {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub arguments: Option<String>,
}

/// High-level stream events, normalized for easier consumption.
#[derive(Debug, Clone)]
pub enum StreamEvent {
    /// First chunk arrived, contains model info.
    MessageStart { id: String, model: String },
    /// Text content delta.
    ContentDelta { text: String },
    /// A new tool call started (index, id, function name).
    ToolCallStart {
        index: usize,
        id: String,
        name: String,
    },
    /// Argument fragment for a tool call being streamed.
    ToolCallDelta { index: usize, arguments: String },
    /// Stream finished with a reason ("stop", "tool_calls", "length").
    Done { finish_reason: String },
    /// Usage info (sent when `stream_options.include_usage` is enabled).
    Usage(Usage),
}

#[derive(Debug, Deserialize)]
struct ApiErrorResponse {
    error: ApiErrorDetail,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ApiErrorDetail {
    pub message: String,
    #[serde(rename = "type")]
    pub error_type: Option<String>,
}

// --- Config ---

#[derive(Debug, Clone)]
pub struct OpenAIConfig {
    pub api_key: String,
    pub model: String,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
    pub timeout: Duration,
}

impl Default for OpenAIConfig {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            model: "gpt-4o".to_string(),
            max_tokens: Some(4096),
            temperature: Some(1.0),
            timeout: Duration::from_secs(120),
        }
    }
}

// --- Client ---

#[derive(Debug)]
pub struct OpenAIClient {
    client: Client,
    config: OpenAIConfig,
    base_url: String,
}

pub struct ClientRequest {
    messages: Vec<Message>,
    system: Option<String>,
    model: Option<String>,
    tools: Vec<Tool>,
}

impl ClientRequest {
    pub fn new(messages: Vec<Message>) -> Self {
        ClientRequest {
            messages,
            system: None,
            model: None,
            tools: vec![],
        }
    }

    pub fn with_system(mut self, system: String) -> Self {
        self.system = Some(system);
        self
    }

    pub fn with_model(mut self, model: String) -> Self {
        self.model = Some(model);
        self
    }

    pub fn with_tools(mut self, tools: Vec<Tool>) -> Self {
        self.tools = tools;
        self
    }

    /// Accept claude::Tool vec and convert automatically.
    pub fn with_claude_tools(mut self, tools: Vec<crate::claude::Tool>) -> Self {
        self.tools = tools.into_iter().map(Tool::from).collect();
        self
    }

    fn build_messages(&self) -> Vec<Message> {
        let mut msgs = Vec::new();
        if let Some(system) = &self.system {
            msgs.push(Message {
                role: Role::System,
                content: Some(system.clone()),
                tool_calls: None,
                tool_call_id: None,
            });
        }
        msgs.extend(self.messages.clone());
        msgs
    }
}

impl OpenAIClient {
    const BASE_URL: &'static str = "https://api.openai.com/v1";

    pub fn new(config: OpenAIConfig) -> OpenAIResult<Self> {
        if config.api_key.is_empty() {
            return Err(OpenAIError::Config("API key is required".to_string()));
        }

        let mut headers = header::HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            header::HeaderValue::from_str(&format!("Bearer {}", config.api_key))
                .map_err(|_| OpenAIError::Config("Invalid API key format".to_string()))?,
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

    pub fn with_base_url(mut self, base_url: String) -> Self {
        self.base_url = base_url;
        self
    }

    pub async fn chat(&self, req: ClientRequest) -> OpenAIResult<ChatCompletionResponse> {
        let url = format!("{}/chat/completions", self.base_url);
        let messages = req.build_messages();

        let inner = ChatCompletionRequest {
            model: req.model.unwrap_or_else(|| self.config.model.clone()),
            messages,
            temperature: self.config.temperature,
            max_tokens: self.config.max_tokens,
            tools: req.tools,
            stream: false,
        };

        let response = self.client.post(&url).json(&inner).send().await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(OpenAIError::ApiError {
                message: format!("{status}: {body}"),
            });
        }

        let chat_response: ChatCompletionResponse = response.json().await?;
        Ok(chat_response)
    }

    pub async fn chat_stream(
        &self,
        req: ClientRequest,
    ) -> Result<impl Stream<Item = OpenAIResult<StreamEvent>> + Send + 'static, anyhow::Error> {
        let url = format!("{}/chat/completions", self.base_url);
        let client = self.client.clone();

        let messages = req.build_messages();
        let request = ChatCompletionRequest {
            model: req.model.unwrap_or_else(|| self.config.model.clone()),
            messages,
            temperature: self.config.temperature,
            max_tokens: self.config.max_tokens,
            tools: req.tools,
            stream: true,
        };

        let initial = client.post(&url).json(&request).send().await?;

        if !initial.status().is_success() {
            let status = initial.status();
            let body = initial.text().await.unwrap_or_default();
            return Err(anyhow!("API error {status}: {body}"));
        }

        let mut byte_stream = initial.bytes_stream();
        let mut buffer = String::new();
        let mut seen_first = false;

        let stream = try_stream! {
            while let Some(chunk) = byte_stream.next().await {
                let chunk = chunk?;
                buffer.push_str(&String::from_utf8_lossy(&chunk));

                while let Some(chunk) = extract_sse_event(&mut buffer)? {
                    if !seen_first {
                        seen_first = true;
                        yield StreamEvent::MessageStart {
                            id: chunk.id.clone(),
                            model: chunk.model.clone(),
                        };
                    }

                    for choice in &chunk.choices {
                        if let Some(ref text) = choice.delta.content {
                            if !text.is_empty() {
                                yield StreamEvent::ContentDelta { text: text.clone() };
                            }
                        }

                        if let Some(ref tool_calls) = choice.delta.tool_calls {
                            for tc in tool_calls {
                                if let Some(ref func) = tc.function {
                                    if let (Some(id), Some(name)) = (&tc.id, &func.name) {
                                        yield StreamEvent::ToolCallStart {
                                            index: tc.index,
                                            id: id.clone(),
                                            name: name.clone(),
                                        };
                                    }
                                    if let Some(ref args) = func.arguments {
                                        if !args.is_empty() {
                                            yield StreamEvent::ToolCallDelta {
                                                index: tc.index,
                                                arguments: args.clone(),
                                            };
                                        }
                                    }
                                }
                            }
                        }

                        if let Some(ref reason) = choice.finish_reason {
                            yield StreamEvent::Done { finish_reason: reason.clone() };
                        }
                    }

                    if let Some(usage) = chunk.usage {
                        yield StreamEvent::Usage(usage);
                    }
                }
            }
        };

        Ok(stream)
    }
}

fn extract_sse_event(buffer: &mut String) -> OpenAIResult<Option<StreamChunk>> {
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

    if data_line.is_empty() {
        return Ok(None);
    }

    if data_line == "[DONE]" {
        return Ok(None);
    }

    match serde_json::from_str::<StreamChunk>(&data_line) {
        Ok(chunk) => Ok(Some(chunk)),
        Err(e) => Err(OpenAIError::Serialization(e)),
    }
}
