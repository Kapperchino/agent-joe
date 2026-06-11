use crate::llm;
use crate::llm::{ClientResponse, LLmClientTrait};
use crate::openai_config::{OpenAIAuthConfig, OpenAIConfig, OpenAIEffort};
use anyhow::{anyhow, Error};
use async_stream::try_stream;
use futures::{Stream, StreamExt};
use reqwest::{header, Client};
use reqwest_middleware::{ClientBuilder, ClientWithMiddleware};
use reqwest_retry::policies::ExponentialBackoff;
use reqwest_retry::RetryTransientMiddleware;
use reqwest_tracing::TracingMiddleware;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::future::ready;
use std::str::FromStr;
use std::time::Duration;
use thiserror::Error;
use tracing::info;
use utils::utils::FnvHashMap;

const HTTP_MAX_RETRIES: u32 = 5;

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
#[serde(untagged)]
pub enum Tool {
    Function {
        #[serde(rename = "type")]
        tool_type: String,
        name: String,
        description: String,
        parameters: FunctionParameters,
    },
    WebSearch {
        #[serde(rename = "type")]
        tool_type: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionParameters {
    #[serde(rename = "type")]
    pub param_type: String,
    pub properties: FnvHashMap<String, ToolProperty>,
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
        properties: FnvHashMap<String, ToolProperty>,
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
    pub parallel_tool_calls: bool,
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
    #[serde(rename = "web_search_call")]
    WebSearchCall {
        id: String,
        #[serde(default)]
        status: Option<String>,
        #[serde(default)]
        action: Option<WebSearchAction>,
    },
}

#[derive(Debug, Deserialize, Clone)]
#[serde(tag = "type")]
pub enum ContentPart {
    #[serde(rename = "output_text")]
    OutputText { text: String },
}

#[derive(Debug, Deserialize, Clone)]
#[serde(tag = "type")]
pub enum WebSearchAction {
    #[serde(rename = "search")]
    Search {
        #[serde(default)]
        query: Option<String>,
        #[serde(default)]
        queries: Vec<String>,
        #[serde(default)]
        domains: Vec<String>,
        #[serde(default)]
        sources: Vec<WebSearchSource>,
    },
    #[serde(rename = "open_page")]
    OpenPage { url: String },
    #[serde(rename = "find_in_page", alias = "find")]
    Find { url: String, pattern: String },
}

#[derive(Debug, Deserialize, Clone)]
pub struct WebSearchSource {
    #[serde(rename = "type")]
    pub source_type: String,
    pub url: String,
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
    #[serde(rename = "web_search_call")]
    WebSearchCall {
        #[serde(default)]
        id: Option<String>,
        #[serde(default)]
        status: Option<String>,
        #[serde(default)]
        action: Option<WebSearchAction>,
    },
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ReasoningText {
    #[serde(rename = "type")]
    prop_type: String,
    #[serde(default)]
    text: String,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(tag = "type")]
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
    #[serde(rename = "response.keepalive")]
    KeepAlive {
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
    #[serde(rename = "response.web_search_call.in_progress")]
    WebSearchCallInProgress {
        #[serde(default)]
        item_id: String,
        #[serde(default)]
        output_index: usize,
        #[serde(default)]
        sequence_number: u64,
    },
    #[serde(rename = "response.web_search_call.searching")]
    WebSearchCallSearching {
        #[serde(default)]
        item_id: String,
        #[serde(default)]
        output_index: usize,
        #[serde(default)]
        sequence_number: u64,
    },
    #[serde(rename = "response.web_search_call.completed")]
    WebSearchCallCompleted {
        #[serde(default)]
        item_id: String,
        #[serde(default)]
        output_index: usize,
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

    #[serde(rename = "response.reasoning_text.delta")]
    ReasoningTextDelta {
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
    #[serde(rename = "response.reasoning_text.done")]
    ReasoningTextDone {
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
}

#[derive(Debug, Clone, Serialize)]
pub struct ReasoningConfig {
    pub effort: OpenAIEffort,
    pub summary: ReasoningSummary,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningSummary {
    Auto,
    Concise,
    Detailed,
}

#[derive(Debug, Clone)]
pub struct OpenAIClient {
    client: ClientWithMiddleware,
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
    pub fn new(config: OpenAIConfig) -> anyhow::Result<Self> {
        let headers = match &config.auth {
            OpenAIAuthConfig::APIKey(api) => {
                let mut headers = header::HeaderMap::new();
                headers.insert(
                    header::AUTHORIZATION,
                    header::HeaderValue::from_str(&format!("Bearer {}", api.api_key))
                        .map_err(|_| OpenAIError::Config("Invalid API key format".to_string()))?,
                );
                headers
            }
            OpenAIAuthConfig::Codex(codex) => {
                let mut headers = header::HeaderMap::new();
                headers.insert(
                    header::AUTHORIZATION,
                    header::HeaderValue::from_str(&format!("Bearer {}", codex.access_token))
                        .map_err(|_| OpenAIError::Config("Invalid API key format".to_string()))?,
                );
                headers.insert(
                    header::CONTENT_TYPE,
                    header::HeaderValue::from_static("application/json"),
                );
                headers.insert(
                    header::HeaderName::from_str("ChatGPT-Account-Id")?,
                    header::HeaderValue::from_str(codex.account_id.as_str())?,
                );
                headers.insert(
                    header::HeaderName::from_static("openai-beta"),
                    header::HeaderValue::from_static("responses=experimental"),
                );
                headers.insert(
                    header::HeaderName::from_static("originator"),
                    header::HeaderValue::from_static("codex_cli_rs"),
                );
                headers
            }
            OpenAIAuthConfig::Local(local) => {
                let mut headers = header::HeaderMap::new();
                if let Some(api_key) = local.api_key.as_ref().filter(|key| !key.trim().is_empty()) {
                    headers.insert(
                        header::AUTHORIZATION,
                        header::HeaderValue::from_str(&format!("Bearer {}", api_key)).map_err(
                            |_| OpenAIError::Config("Invalid API key format".to_string()),
                        )?,
                    );
                }
                headers
            }
            OpenAIAuthConfig::OpenRouter(openrouter) => {
                let mut headers = header::HeaderMap::new();
                headers.insert(
                    header::AUTHORIZATION,
                    header::HeaderValue::from_str(&format!("Bearer {}", openrouter.api_key))
                        .map_err(|_| OpenAIError::Config("Invalid API key format".to_string()))?,
                );
                headers
            }
        };

        let retry_policy = ExponentialBackoff::builder().build_with_max_retries(HTTP_MAX_RETRIES);
        let inner_client = Client::builder()
            .connect_timeout(Duration::from_secs(60))
            .read_timeout(Duration::from_secs(300))
            .default_headers(headers)
            .build()?;

        let client = ClientBuilder::new(inner_client)
            .with(TracingMiddleware::default())
            .with(RetryTransientMiddleware::new_with_policy(retry_policy))
            .build();

        Ok(Self { client, config })
    }

    pub async fn chat(&self, req: ClientRequest) -> OpenAIResult<Response> {
        let url = format!("{}/responses", self.config.get_url());

        let inner = ResponseRequest {
            model: req.model.unwrap_or_else(|| self.config.model.clone()),
            input: req.input,
            instructions: req.instructions.unwrap_or_default(),
            temperature: None,
            max_output_tokens: None,
            tools: req.tools,
            reasoning: Some(self.config.get_reasoning()),
            parallel_tool_calls: true,
            stream: false,
            store: false,
        };

        let response = self
            .client
            .post(&url)
            .json(&inner)
            .send()
            .await
            .map_err(|e| OpenAIError::ApiError {
                message: e.to_string(),
            })?;

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
        let url = format!("{}/responses", self.config.get_url());

        let request = ResponseRequest {
            model: req.model.unwrap_or_else(|| self.config.model.clone()),
            input: req.input,
            instructions: req.instructions.unwrap_or_default(),
            temperature: None,
            max_output_tokens: None,
            tools: req.tools,
            reasoning: Some(self.config.get_reasoning()),
            parallel_tool_calls: true,
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
                    info!("{:?}", event);
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

        let mut data_line = String::new();

        for line in event_text.lines() {
            let line = line.trim();
            if let Some(data) = line.strip_prefix("data: ") {
                data_line.push_str(data);
            }
        }

        if data_line.is_empty() || data_line == "[DONE]" {
            return Ok(None);
        }

        match serde_json::from_str::<StreamEvent>(&data_line) {
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
