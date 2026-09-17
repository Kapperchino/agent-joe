use crate::llm;
use crate::llm::{ClientResponse, LLmClientTrait};
use crate::openai_config::{OpenAIAuthConfig, OpenAIConfig, OpenAIEffort};
use anyhow::{Error, anyhow};
use futures::{Stream, StreamExt};
use reqwest::{Client, header};
use reqwest_middleware::{ClientBuilder, ClientWithMiddleware};
use reqwest_retry::RetryTransientMiddleware;
use reqwest_retry::policies::ExponentialBackoff;
use reqwest_tracing::TracingMiddleware;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::future::ready;
use std::str::FromStr;
use std::time::Duration;
use thiserror::Error;
use tools::tool_defs::NonEmptyString;

const HTTP_MAX_RETRIES: u32 = 5;

mod prompt_cache;
pub use prompt_cache::InputContent;
use prompt_cache::PromptCacheOptions;

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
    Message {
        role: Role,
        content: InputContent,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        phase: Option<llm::MessagePhase>,
    },
    #[serde(rename = "function_call")]
    FunctionCall {
        id: NonEmptyString,
        call_id: NonEmptyString,
        name: NonEmptyString,
        arguments: String,
    },
    #[serde(rename = "function_call_output")]
    FunctionCallOutput {
        call_id: NonEmptyString,
        output: InputContent,
    },
    #[serde(rename = "reasoning")]
    Reasoning(ReasoningItem),
    #[serde(rename = "compaction_trigger")]
    CompactionTrigger,
    #[serde(untagged)]
    Native(serde_json::Value),
}

impl InputItem {
    pub fn user(content: String) -> Self {
        InputItem::Message {
            role: Role::User,
            content: content.into(),
            phase: None,
        }
    }

    pub fn assistant(content: String) -> Self {
        InputItem::Message {
            role: Role::Assistant,
            content: content.into(),
            phase: None,
        }
    }

    pub fn function_call_output(call_id: NonEmptyString, output: String) -> Self {
        InputItem::FunctionCallOutput {
            call_id,
            output: output.into(),
        }
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
    pub properties: BTreeMap<String, ToolProperty>,
    pub required: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ToolProperty {
    Schema(serde_json::Value),
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
        properties: BTreeMap<String, ToolProperty>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResponseInclude {
    #[serde(rename = "reasoning.encrypted_content")]
    EncryptedReasoning,
}

#[derive(Clone, Copy)]
enum ResponseMode {
    Complete,
    Streaming,
}

#[derive(Debug, Serialize)]
struct ResponseRequest {
    pub model: String,
    pub input: Vec<InputItem>,
    pub instructions: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_cache_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_cache_options: Option<PromptCacheOptions>,
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
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub include: Vec<ResponseInclude>,
}

impl ResponseRequest {
    fn compaction(config: &OpenAIConfig, mut request: ClientRequest) -> Self {
        request.input.push(InputItem::CompactionTrigger);
        Self::new(config, request, ResponseMode::Streaming)
    }

    fn new(config: &OpenAIConfig, req: ClientRequest, mode: ResponseMode) -> Self {
        let model = req.model.unwrap_or_else(|| config.model.clone());
        let prompt_cache_options = PromptCacheOptions::for_model(config, &model);
        let input = req
            .input
            .into_iter()
            .map(|item| match &prompt_cache_options {
                Some(_) => item.with_cache_breakpoint(),
                None => item,
            })
            .collect();
        Self {
            model,
            input,
            instructions: req.instructions.unwrap_or_default(),
            prompt_cache_key: req
                .prompt_cache_key
                .filter(|_| config.supports_prompt_cache_key()),
            prompt_cache_options,
            temperature: None,
            max_output_tokens: match config.auth {
                OpenAIAuthConfig::Codex(_) => None,
                _ => req.max_output_tokens,
            },
            tools: req.tools,
            reasoning: Some(config.get_reasoning()),
            parallel_tool_calls: true,
            stream: matches!(mode, ResponseMode::Streaming),
            store: false,
            include: config.reasoning_include(),
        }
    }
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

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ReasoningItem {
    pub id: String,
    #[serde(default)]
    pub summary: Vec<SummaryTextContent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encrypted_content: Option<String>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(tag = "type")]
pub enum OutputItem {
    #[serde(rename = "message")]
    Message {
        id: String,
        content: Vec<ContentPart>,
        #[serde(default)]
        phase: Option<llm::MessagePhase>,
    },
    #[serde(rename = "function_call")]
    FunctionCall {
        id: NonEmptyString,
        call_id: NonEmptyString,
        name: NonEmptyString,
        arguments: String,
    },
    #[serde(rename = "reasoning")]
    Reasoning(ReasoningItem),
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
    pub input_tokens_details: InputTokenDetails,
    #[serde(default)]
    pub output_tokens_details: OutputTokenDetails,
    #[serde(default)]
    pub input_tokens: u32,
    #[serde(default)]
    pub output_tokens: u32,
    #[serde(default)]
    pub total_tokens: u32,
}

#[derive(Debug, Deserialize, Clone, Default)]
#[serde(default)]
pub struct InputTokenDetails {
    pub cached_tokens: u32,
    pub cache_write_tokens: u32,
}

#[derive(Debug, Deserialize, Clone, Default)]
#[serde(default)]
pub struct OutputTokenDetails {
    pub reasoning_tokens: u32,
}

impl From<Usage> for llm::UsageDelta {
    fn from(usage: Usage) -> Self {
        Self {
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            cached_input_tokens: usage.input_tokens_details.cached_tokens,
            reasoning_tokens: usage.output_tokens_details.reasoning_tokens,
        }
    }
}

impl Usage {
    fn cache_hit_percent(&self) -> f64 {
        match self.input_tokens {
            0 => 0.0,
            input => f64::from(self.input_tokens_details.cached_tokens) * 100.0 / f64::from(input),
        }
    }

    fn log(
        &self,
        response_id: &str,
        model: &str,
        cache_key: Option<&str>,
        purpose: llm::RequestPurpose,
    ) {
        tracing::debug!(
            response_id,
            model,
            prompt_cache_key = cache_key,
            ?purpose,
            input_tokens = self.input_tokens,
            cached_input_tokens = self.input_tokens_details.cached_tokens,
            cache_write_tokens = self.input_tokens_details.cache_write_tokens,
            uncached_input_tokens = self
                .input_tokens
                .saturating_sub(self.input_tokens_details.cached_tokens),
            cache_hit_percent = self.cache_hit_percent(),
            output_tokens = self.output_tokens,
            reasoning_tokens = self.output_tokens_details.reasoning_tokens,
            total_tokens = self.input_tokens.saturating_add(self.output_tokens),
            "Provider response usage"
        );
    }
}

#[derive(Debug, Default, Serialize, Deserialize, Clone)]
pub struct ResponseError {
    pub code: Option<String>,
    #[serde(default, rename = "type")]
    pub error_type: Option<String>,
    #[serde(default)]
    pub message: String,
    #[serde(flatten)]
    pub details: serde_json::Map<String, serde_json::Value>,
}

impl ResponseError {
    pub(crate) fn api_error(self) -> llm::ApiErrorDetail {
        let codes = [self.code.as_deref(), self.error_type.as_deref()];
        let code = codes
            .iter()
            .flatten()
            .copied()
            .find(|code| matches!(*code, "usage_limit_reached" | "insufficient_quota"))
            .or(self.code.as_deref())
            .or(self.error_type.as_deref())
            .unwrap_or("failed_response");
        llm::ApiErrorDetail {
            error_type: code.to_owned(),
            message: serde_json::to_string(&self).unwrap_or(self.message),
        }
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct IncompleteDetails {
    pub reason: String,
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
    #[serde(default)]
    pub error: Option<ResponseError>,
    #[serde(default)]
    pub incomplete_details: Option<IncompleteDetails>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(tag = "type")]
pub enum StreamOutputItem {
    #[serde(rename = "function_call")]
    FunctionCall {
        #[serde(default)]
        id: Option<NonEmptyString>,
        call_id: NonEmptyString,
        name: NonEmptyString,
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
        error: Option<ResponseError>,
        #[serde(flatten)]
        details: serde_json::Map<String, serde_json::Value>,
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
    pub config: OpenAIConfig,
}
#[derive(Serialize)]
pub struct ClientRequest {
    pub input: Vec<InputItem>,
    pub instructions: Option<String>,
    pub model: Option<String>,
    pub tools: Vec<Tool>,
    pub max_output_tokens: Option<u32>,
    pub prompt_cache_key: Option<String>,
    #[serde(skip)]
    pub purpose: llm::RequestPurpose,
}

impl ClientRequest {
    pub fn new(input: Vec<InputItem>) -> Self {
        ClientRequest {
            input,
            instructions: None,
            model: None,
            tools: vec![],
            max_output_tokens: None,
            prompt_cache_key: None,
            purpose: llm::RequestPurpose::default(),
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
    pub async fn compact(
        &self,
        request: llm::ClientRequest,
    ) -> anyhow::Result<crate::compaction::CompactionResponse> {
        let cache_key = request.prompt_cache_key.clone();
        let model = request
            .model
            .clone()
            .unwrap_or_else(|| self.config.model.clone());
        let request: ClientRequest = request.try_into()?;
        let response = match self.config.auth {
            OpenAIAuthConfig::Codex(_) => {
                let response = self
                    .stream_response(ResponseRequest::compaction(&self.config, request))
                    .await?;
                crate::compaction::CompactionResponse::from_stream(crate::sse::decode(
                    response.bytes_stream(),
                    crate::compaction::CompactionEvent::terminal,
                ))
                .await
            }
            _ => self.compact_standalone(request).await,
        }?;
        response.usage.log(
            "",
            &model,
            cache_key.as_deref(),
            llm::RequestPurpose::Compaction,
        );
        Ok(response)
    }

    async fn compact_standalone(
        &self,
        request: ClientRequest,
    ) -> anyhow::Result<crate::compaction::CompactionResponse> {
        let body = crate::compaction::CompactionRequest {
            model: request.model.unwrap_or_else(|| self.config.model.clone()),
            input: request.input,
            instructions: request.instructions.unwrap_or_default(),
            prompt_cache_key: request
                .prompt_cache_key
                .filter(|_| self.config.supports_prompt_cache_key()),
        };
        let response = self
            .client
            .post(format!(
                "{}/responses/compact",
                self.config.get_url().trim_end_matches('/')
            ))
            .json(&body)
            .send()
            .await?;
        let status = response.status();
        let mut stream = response.bytes_stream();
        let mut bytes = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            match bytes.len().saturating_add(chunk.len()) <= 16 * 1024 * 1024 {
                true => bytes.extend_from_slice(&chunk),
                false => Err(anyhow!("Compaction response exceeds 16 MiB"))?,
            }
        }
        match status.is_success() {
            true => Ok(serde_json::from_slice(&bytes)?),
            false => Err(crate::failure::Failure::http(
                status.as_u16(),
                String::from_utf8_lossy(&bytes).into_owned(),
            )
            .into()),
        }
    }

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

        let builder = ClientBuilder::new(inner_client).with(TracingMiddleware::default());
        let client = match config.auth {
            OpenAIAuthConfig::Codex(_) => builder.build(),
            _ => builder
                .with(RetryTransientMiddleware::new_with_policy(retry_policy))
                .build(),
        };

        Ok(Self { client, config })
    }

    pub async fn chat(&self, req: ClientRequest) -> OpenAIResult<Response> {
        let url = format!("{}/responses", self.config.get_url());

        let purpose = req.purpose;
        let inner = ResponseRequest::new(&self.config, req, ResponseMode::Complete);
        inner.log_cache_fingerprint();

        let response = self
            .client
            .post(&url)
            .json(&inner)
            .send()
            .await
            .map_err(|e| OpenAIError::ApiError {
                message: e.to_string(),
            })?;

        if response.status().is_success() {
            let response: Response = response.json().await?;
            if let Some(usage) = &response.usage {
                usage.log(
                    &response.id,
                    &response.model,
                    inner.prompt_cache_key.as_deref(),
                    purpose,
                );
            }
            Ok(response)
        } else {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            Err(OpenAIError::ApiError {
                message: format!("{status}: {body}"),
            })
        }
    }

    pub async fn chat_stream_openai(
        &self,
        req: ClientRequest,
    ) -> Result<impl Stream<Item = anyhow::Result<StreamEvent>> + Send + 'static, anyhow::Error>
    {
        let purpose = req.purpose;
        let request = ResponseRequest::new(&self.config, req, ResponseMode::Streaming);
        let cache_key = request.prompt_cache_key.clone();
        let response = self.stream_response(request).await?;
        Ok(crate::sse::decode(response.bytes_stream(), |event| {
            matches!(
                event,
                StreamEvent::ResponseCompleted { .. }
                    | StreamEvent::ResponseIncomplete { .. }
                    | StreamEvent::ResponseFailed { .. }
                    | StreamEvent::Error { .. }
            )
        })
        .inspect(move |event| {
            let response = match event {
                Ok(
                    StreamEvent::ResponseCompleted { response, .. }
                    | StreamEvent::ResponseFailed { response, .. }
                    | StreamEvent::ResponseIncomplete { response, .. },
                ) => Some(response),
                _ => None,
            };
            if let Some(response) = response
                && let Some(usage) = &response.usage
            {
                usage.log(&response.id, &response.model, cache_key.as_deref(), purpose);
            }
        }))
    }

    async fn stream_response(&self, request: ResponseRequest) -> anyhow::Result<reqwest::Response> {
        request.log_cache_fingerprint();
        let url = format!("{}/responses", self.config.get_url().trim_end_matches('/'));
        let response = self.client.post(&url).json(&request).send().await?;
        match response.status().is_success() {
            true => Ok(response),
            false => {
                let status = response.status();
                let body = response.text().await.unwrap_or_default();
                Err(crate::failure::Failure::http(status.as_u16(), body).into())
            }
        }
    }
}

impl LLmClientTrait for OpenAIClient {
    async fn chat_stream(
        &self,
        req: llm::ClientRequest,
    ) -> Result<impl Stream<Item = anyhow::Result<llm::StreamEvent>> + Send + 'static, Error> {
        let request = req.try_into().map_err(|error: anyhow::Error| {
            crate::failure::Failure::new(
                crate::failure::FailureKind::InvalidInput,
                error.to_string(),
            )
        })?;
        match self.chat_stream_openai(request).await {
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
#[path = "../tests/unit/openai/request_tests.rs"]
mod request_tests;
