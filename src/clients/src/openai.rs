use crate::llm;
use crate::llm::{ClientResponse, LLmClientTrait};
use anyhow::{anyhow, Error};
use async_stream::try_stream;
use futures::{Stream, StreamExt};
use reqwest::{header, Client};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::future::ready;
use std::time::Duration;
use thiserror::Error;
use tracing::{error, warn};

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
    User,
    Assistant,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum InputItem {
    #[serde(rename = "message")]
    Message { role: Role, content: String },
    #[serde(rename = "function_call")]
    FunctionCall {
        id: String,
        call_id: String,
        name: String,
        arguments: String,
    },
    #[serde(rename = "function_call_output")]
    FunctionCallOutput { call_id: String, output: String },
    #[serde(rename = "reasoning")]
    Reasoning {
        id: String,
        #[serde(default)]
        summary: Vec<SummaryTextContent>,
    },
}

impl InputItem {
    pub fn user(content: String) -> Self {
        InputItem::Message {
            role: Role::User,
            content,
        }
    }

    pub fn assistant(content: String) -> Self {
        InputItem::Message {
            role: Role::Assistant,
            content,
        }
    }

    pub fn function_call_output(call_id: String, output: String) -> Self {
        InputItem::FunctionCallOutput { call_id, output }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tool {
    #[serde(rename = "type")]
    pub tool_type: String,
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

#[derive(Debug, Serialize)]
struct ResponseRequest {
    pub model: String,
    pub input: Vec<InputItem>,
    pub instructions: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<Tool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<ReasoningConfig>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub stream: bool,
    pub store: bool,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Response {
    pub id: String,
    pub model: String,
    pub status: String,
    pub output: Vec<OutputItem>,
    #[serde(default)]
    pub usage: Option<Usage>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SummaryTextContent {
    #[serde(default)]
    pub text: String,
    #[serde(rename = "type")]
    pub prop_type: String,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(tag = "type")]
pub enum OutputItem {
    #[serde(rename = "message")]
    Message {
        id: String,
        content: Vec<ContentPart>,
    },
    #[serde(rename = "function_call")]
    FunctionCall {
        id: String,
        call_id: String,
        name: String,
        arguments: String,
    },
    #[serde(rename = "reasoning")]
    Reasoning {
        id: String,
        #[serde(default)]
        summary: Vec<SummaryTextContent>,
        #[serde(default)]
        content: Vec<SummaryTextContent>,
        #[serde(default)]
        encrypted_content: String,
        #[serde(default)]
        status: String,
    },
}

#[derive(Debug, Deserialize, Clone)]
#[serde(tag = "type")]
pub enum ContentPart {
    #[serde(rename = "output_text")]
    OutputText { text: String },
}

#[derive(Debug, Deserialize, Clone, Default)]
pub struct Usage {
    #[serde(default)]
    pub input_tokens: u32,
    #[serde(default)]
    pub output_tokens: u32,
    #[serde(default)]
    pub total_tokens: u32,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ResponseEnvelope {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub usage: Option<Usage>,
    #[serde(default)]
    pub output: Vec<OutputItem>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(tag = "type")]
pub enum StreamOutputItem {
    #[serde(rename = "function_call")]
    FunctionCall {
        #[serde(default)]
        id: Option<String>,
        #[serde(default)]
        call_id: String,
        #[serde(default)]
        name: String,
    },
    #[serde(rename = "message")]
    Message {
        #[serde(default)]
        id: Option<String>,
        #[serde(default)]
        role: Option<String>,
    },
    #[serde(rename = "reasoning")]
    Reasoning {
        #[serde(default)]
        summary_text_content: Vec<SummaryTextContent>,
        #[serde(default)]
        content: Vec<ReasoningText>,
    },
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ReasoningText {
    #[serde(rename = "type")]
    prop_type: String,
    #[serde(default)]
    text: String,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(tag = "e_type")]
pub enum StreamEvent {
    #[serde(rename = "response.queued")]
    ResponseQueued {
        response: ResponseEnvelope,
        #[serde(default)]
        sequence_number: u64,
    },
    #[serde(rename = "response.created")]
    ResponseCreated {
        response: ResponseEnvelope,
        #[serde(default)]
        sequence_number: u64,
    },
    #[serde(rename = "response.in_progress")]
    ResponseInProgress {
        #[serde(default)]
        response: Option<ResponseEnvelope>,
        #[serde(default)]
        sequence_number: u64,
    },
    #[serde(rename = "response.completed")]
    ResponseCompleted {
        response: ResponseEnvelope,
        #[serde(default)]
        sequence_number: u64,
    },
    #[serde(rename = "response.incomplete")]
    ResponseIncomplete {
        response: ResponseEnvelope,
        #[serde(default)]
        sequence_number: u64,
    },
    #[serde(rename = "response.failed")]
    ResponseFailed {
        response: ResponseEnvelope,
        #[serde(default)]
        sequence_number: u64,
    },
    #[serde(rename = "response.output_item.added")]
    OutputItemAdded {
        output_index: usize,
        item: StreamOutputItem,
        #[serde(default)]
        sequence_number: u64,
    },
    #[serde(rename = "response.output_item.done")]
    OutputItemDone {
        output_index: usize,
        #[serde(default)]
        item: Option<OutputItem>,
        #[serde(default)]
        sequence_number: u64,
    },
    #[serde(rename = "response.content_part.added")]
    ContentPartAdded {
        #[serde(default)]
        item_id: String,
        #[serde(default)]
        output_index: usize,
        #[serde(default)]
        content_index: usize,
        #[serde(default)]
        sequence_number: u64,
    },
    #[serde(rename = "response.content_part.done")]
    ContentPartDone {
        #[serde(default)]
        item_id: String,
        #[serde(default)]
        output_index: usize,
        #[serde(default)]
        content_index: usize,
        #[serde(default)]
        sequence_number: u64,
    },

    #[serde(rename = "response.output_text.delta")]
    OutputTextDelta {
        #[serde(default)]
        item_id: String,
        #[serde(default)]
        output_index: usize,
        #[serde(default)]
        content_index: usize,
        delta: String,
        #[serde(default)]
        sequence_number: u64,
    },
    #[serde(rename = "response.output_text.done")]
    OutputTextDone {
        #[serde(default)]
        item_id: String,
        #[serde(default)]
        output_index: usize,
        #[serde(default)]
        content_index: usize,
        text: String,
        #[serde(default)]
        sequence_number: u64,
    },
    #[serde(rename = "response.output_text.annotation.added")]
    OutputTextAnnotationAdded {
        #[serde(default)]
        item_id: String,
        #[serde(default)]
        output_index: usize,
        #[serde(default)]
        content_index: usize,
        #[serde(default)]
        annotation_index: Option<usize>,
        #[serde(default)]
        annotation: Option<serde_json::Value>,
        #[serde(default)]
        sequence_number: u64,
    },

    #[serde(rename = "response.refusal.delta")]
    RefusalDelta {
        #[serde(default)]
        item_id: String,
        #[serde(default)]
        output_index: usize,
        #[serde(default)]
        content_index: usize,
        delta: String,
        #[serde(default)]
        sequence_number: u64,
    },
    #[serde(rename = "response.refusal.done")]
    RefusalDone {
        #[serde(default)]
        item_id: String,
        #[serde(default)]
        output_index: usize,
        #[serde(default)]
        content_index: usize,
        refusal: String,
        #[serde(default)]
        sequence_number: u64,
    },

    #[serde(rename = "response.function_call_arguments.delta")]
    FunctionCallArgumentsDelta {
        #[serde(default)]
        item_id: String,
        #[serde(default)]
        output_index: usize,
        delta: String,
        #[serde(default)]
        sequence_number: u64,
    },
    #[serde(rename = "response.function_call_arguments.done")]
    FunctionCallArgumentsDone {
        #[serde(default)]
        item_id: String,
        #[serde(default)]
        output_index: usize,
        #[serde(default)]
        name: String,
        #[serde(default)]
        arguments: String,
        #[serde(default)]
        sequence_number: u64,
    },

    #[serde(rename = "response.reasoning_summary_part.added")]
    ReasoningSummaryPartAdded {
        #[serde(default)]
        sequence_number: u64,
    },
    #[serde(rename = "response.reasoning_summary_text.delta")]
    ReasoningSummaryTextDelta {
        #[serde(default)]
        item_id: String,
        #[serde(default)]
        output_index: usize,
        #[serde(default)]
        summary_index: Option<usize>,
        delta: String,
        #[serde(default)]
        sequence_number: u64,
    },
    #[serde(rename = "response.reasoning_summary_text.done")]
    ReasoningSummaryTextDone {
        #[serde(default)]
        item_id: String,
        #[serde(default)]
        output_index: usize,
        #[serde(default)]
        summary_index: Option<usize>,
        text: String,
        #[serde(default)]
        sequence_number: u64,
    },
    #[serde(rename = "response.reasoning_summary_part.done")]
    ReasoningSummaryPartDone {
        #[serde(default)]
        sequence_number: u64,
    },

    #[serde(rename = "error")]
    Error {
        #[serde(default)]
        code: String,
        #[serde(default)]
        message: String,
        #[serde(default)]
        sequence_number: u64,
    },

    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone)]
pub struct OpenAIConfig {
    pub url: String,
    pub api_key: String,
    pub model: String,
    pub max_output_tokens: Option<u32>,
    pub temperature: Option<f32>,
    pub timeout: Duration,
    pub reasoning: Option<ReasoningConfig>,
    /// Additional headers to include in every request (e.g. ChatGPT-Account-Id).
    pub extra_headers: Vec<(String, String)>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReasoningConfig {
    pub effort: ReasoningEffort,
    pub summary: ReasoningSummary,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningEffort {
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningSummary {
    Auto,
    Concise,
    Detailed,
}

impl Default for OpenAIConfig {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            url: "https://api.openai.com/v1".to_string(),
            model: "gpt-5.4".to_string(),
            max_output_tokens: None,
            temperature: None,
            timeout: Duration::from_secs(120),
            reasoning: Some(ReasoningConfig {
                effort: ReasoningEffort::Medium,
                summary: ReasoningSummary::Auto,
            }),
            extra_headers: vec![],
        }
    }
}

#[derive(Debug)]
pub struct OpenAIClient {
    client: Client,
    config: OpenAIConfig,
}
#[derive(Serialize)]
pub struct ClientRequest {
    pub input: Vec<InputItem>,
    pub instructions: Option<String>,
    pub model: Option<String>,
    pub tools: Vec<Tool>,
}

impl ClientRequest {
    pub fn new(input: Vec<InputItem>) -> Self {
        ClientRequest {
            input,
            instructions: None,
            model: None,
            tools: vec![],
        }
    }

    pub fn with_instructions(mut self, instructions: String) -> Self {
        self.instructions = Some(instructions);
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
}

impl OpenAIClient {
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
        for (name, value) in &config.extra_headers {
            headers.insert(
                header::HeaderName::from_bytes(name.as_bytes())
                    .map_err(|_| OpenAIError::Config(format!("Invalid header name: {name}")))?,
                header::HeaderValue::from_str(value)
                    .map_err(|_| OpenAIError::Config(format!("Invalid header value for {name}")))?,
            );
        }

        let client = Client::builder()
            .timeout(config.timeout)
            .default_headers(headers)
            .build()?;

        Ok(Self { client, config })
    }

    pub async fn chat(&self, req: ClientRequest) -> OpenAIResult<Response> {
        let url = format!("{}/responses", self.config.url);

        let inner = ResponseRequest {
            model: req.model.unwrap_or_else(|| self.config.model.clone()),
            input: req.input,
            instructions: req.instructions.unwrap_or_default(),
            temperature: self.config.temperature,
            max_output_tokens: self.config.max_output_tokens,
            tools: req.tools,
            reasoning: self.config.reasoning.clone(),
            stream: false,
            store: false,
        };

        let response = self.client.post(&url).json(&inner).send().await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(OpenAIError::ApiError {
                message: format!("{status}: {body}"),
            });
        }

        let resp: Response = response.json().await?;
        Ok(resp)
    }

    pub async fn chat_stream_openai(
        &self,
        req: ClientRequest,
    ) -> Result<impl Stream<Item = OpenAIResult<StreamEvent>> + Send + 'static, anyhow::Error> {
        let url = format!("{}/responses", self.config.url);

        let request = ResponseRequest {
            model: req.model.unwrap_or_else(|| self.config.model.clone()),
            input: req.input,
            instructions: req.instructions.unwrap_or_default(),
            temperature: self.config.temperature,
            max_output_tokens: self.config.max_output_tokens,
            tools: req.tools,
            reasoning: self.config.reasoning.clone(),
            stream: true,
            store: false,
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

                while let Some(event) = Self::extract_sse_event(&mut buffer)? {
                                        warn!("{:?}", event);
                    yield event;
                }
            }
        };

        Ok(stream)
    }

    fn extract_sse_event(buffer: &mut String) -> OpenAIResult<Option<StreamEvent>> {
        let delimiter_pos = match buffer.find("\n\n") {
            Some(pos) => pos,
            None => return Ok(None),
        };

        let event_text = buffer[..delimiter_pos].to_string();
        buffer.drain(..=delimiter_pos + 1);

        let mut event_type = String::new();
        let mut data_line = String::new();

        for line in event_text.lines() {
            let line = line.trim();
            if let Some(evt) = line.strip_prefix("event: ") {
                event_type = evt.to_string();
            } else if let Some(data) = line.strip_prefix("data: ") {
                data_line.push_str(data);
            }
        }

        if event_type.is_empty() || data_line.is_empty() {
            return Ok(None);
        }

        let mut json: serde_json::Value = serde_json::from_str(&data_line)?;
        json.as_object_mut()
            .map(|obj| obj.insert("e_type".to_string(), serde_json::Value::String(event_type)));

        match serde_json::from_value::<StreamEvent>(json) {
            Ok(event) => Ok(Some(event)),
            Err(e) => Err(OpenAIError::Serialization(e)),
        }
    }
}

impl LLmClientTrait for OpenAIClient {
    async fn chat_stream(
        &self,
        req: llm::ClientRequest,
    ) -> Result<impl Stream<Item = anyhow::Result<llm::StreamEvent>> + Send + 'static, Error> {
        match self.chat_stream_openai(req.into()).await {
            Ok(stream) => Ok(stream
                .map(|x| match x {
                    Ok(event) => {
                        let converted: Option<llm::StreamEvent> = event.into();
                        converted.map(Ok)
                    }
                    Err(err) => Some(Err(anyhow!(err))),
                })
                .filter_map(ready)),
            Err(e) => Err(anyhow!(e)),
        }
    }

    async fn send_request(
        &self,
        request: crate::llm::ClientRequest,
    ) -> anyhow::Result<ClientResponse> {
        todo!()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::pin::pin;

    #[tokio::test]
    async fn test_chat_api_call() {
        let api_key = std::env::var("OPENAI_KEY").expect("OPENAI_KEY must be set");
        let config = OpenAIConfig {
            api_key,
            ..Default::default()
        };
        let client = OpenAIClient::new(config).unwrap();
        let req = ClientRequest::new(vec![InputItem::user("Say hello".to_string())]);
        let response = client.chat(req).await.unwrap();

        println!("{:?}", response);

        assert_eq!(response.status, "completed");
        assert!(!response.output.is_empty());
    }

    #[tokio::test]
    async fn test_chat_stream_api_call() {
        let api_key = std::env::var("OPENAI_KEY").expect("OPENAI_KEY must be set");
        let config = OpenAIConfig {
            api_key,
            ..Default::default()
        };
        let client = OpenAIClient::new(config).unwrap();
        let req = ClientRequest::new(vec![InputItem::user("Say hello".to_string())]);
        let mut stream = client.chat_stream_openai(req).await.unwrap();

        let mut got_start = false;
        let mut got_content = false;
        let mut got_done = false;

        let mut stream = pin!(stream);

        while let Some(event) = stream.next().await {
            let event = event.unwrap();
            println!("{:?}", event);
            match event {
                StreamEvent::ResponseCreated { .. } => got_start = true,
                StreamEvent::OutputTextDelta { .. } => got_content = true,
                StreamEvent::ResponseCompleted { .. } => got_done = true,
                _ => {}
            }
        }

        assert!(got_start, "should receive ResponseCreated");
        assert!(got_content, "should receive OutputTextDelta");
        assert!(got_done, "should receive ResponseCompleted");
    }

    #[tokio::test]
    async fn test_chat_stream_openrouter_api_call() {
        let api_key = std::env::var("OPEN_KEY").expect("OPEN_KEY must be set");
        let config = OpenAIConfig {
            url: "https://openrouter.ai/api/v1".to_string(),
            model: "openai/gpt-5.2-codex".to_string(),
            api_key,
            ..Default::default()
        };
        let client = OpenAIClient::new(config).unwrap();
        let req = ClientRequest::new(vec![InputItem::user("Say hello".to_string())]);
        let mut stream = client.chat_stream_openai(req).await.unwrap();

        let mut got_start = false;
        let mut got_content = false;
        let mut got_done = false;

        let mut stream = pin!(stream);

        while let Some(event) = stream.next().await {
            let event = event.unwrap();
            println!("{:?}", event);
            match event {
                StreamEvent::ResponseCreated { .. } => got_start = true,
                StreamEvent::OutputTextDelta { .. } => got_content = true,
                StreamEvent::ResponseCompleted { .. } => got_done = true,
                _ => {}
            }
        }

        assert!(got_start, "should receive ResponseCreated");
        assert!(got_content, "should receive OutputTextDelta");
        assert!(got_done, "should receive ResponseCompleted");
    }
}
