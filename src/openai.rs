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
    User,
    Assistant,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum InputItem {
    #[serde(rename = "message")]
    Message { role: Role, content: String },
    #[serde(rename = "function_call_output")]
    FunctionCallOutput { call_id: String, output: String },
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

#[derive(Debug, Serialize)]
struct ResponseRequest {
    pub model: String,
    pub input: Vec<InputItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<Tool>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub stream: bool,
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

#[derive(Debug, Clone)]
pub enum StreamEvent {
    /// First event arrived, contains model info.
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
    /// Stream finished with a reason ("completed", "incomplete", "failed").
    Done { finish_reason: String },
    /// Usage info from the completed response.
    Usage(Usage),
}

// --- Raw streaming event deserialization ---

#[derive(Debug, Deserialize, Clone)]
struct StreamResponseEnvelope {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    usage: Option<Usage>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(tag = "type")]
enum StreamOutputItem {
    #[serde(rename = "function_call")]
    FunctionCall { call_id: String, name: String },
    #[serde(rename = "message")]
    Message {},
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(tag = "type")]
enum RawStreamEvent {
    #[serde(rename = "response.created")]
    ResponseCreated { response: StreamResponseEnvelope },
    #[serde(rename = "response.in_progress")]
    ResponseInProgress {},
    #[serde(rename = "response.output_item.added")]
    OutputItemAdded {
        output_index: usize,
        item: StreamOutputItem,
    },
    #[serde(rename = "response.output_text.delta")]
    OutputTextDelta {
        output_index: usize,
        content_index: usize,
        delta: String,
    },
    #[serde(rename = "response.function_call_arguments.delta")]
    FunctionCallArgumentsDelta { output_index: usize, delta: String },
    #[serde(rename = "response.function_call_arguments.done")]
    FunctionCallArgumentsDone {
        output_index: usize,
        name: String,
        arguments: String,
    },
    #[serde(rename = "response.output_item.done")]
    OutputItemDone {},
    #[serde(rename = "response.output_text.done")]
    OutputTextDone {},
    #[serde(rename = "response.content_part.added")]
    ContentPartAdded {},
    #[serde(rename = "response.content_part.done")]
    ContentPartDone {},
    #[serde(rename = "response.completed")]
    ResponseCompleted { response: StreamResponseEnvelope },
    #[serde(other)]
    Unknown,
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

#[derive(Debug, Clone)]
pub struct OpenAIConfig {
    pub url: String,
    pub api_key: String,
    pub model: String,
    pub max_output_tokens: Option<u32>,
    pub temperature: Option<f32>,
    pub timeout: Duration,
}

impl Default for OpenAIConfig {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            url: "https://api.openai.com/v1".to_string(),
            model: "gpt-4o".to_string(),
            max_output_tokens: Some(4096),
            temperature: Some(1.0),
            timeout: Duration::from_secs(120),
        }
    }
}

#[derive(Debug)]
pub struct OpenAIClient {
    client: Client,
    config: OpenAIConfig,
}

pub struct ClientRequest {
    input: Vec<InputItem>,
    instructions: Option<String>,
    model: Option<String>,
    tools: Vec<Tool>,
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
            instructions: req.instructions,
            temperature: self.config.temperature,
            max_output_tokens: self.config.max_output_tokens,
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

        let resp: Response = response.json().await?;
        Ok(resp)
    }

    pub async fn chat_stream(
        &self,
        req: ClientRequest,
    ) -> Result<impl Stream<Item = OpenAIResult<StreamEvent>> + Send + 'static, anyhow::Error> {
        let url = format!("{}/responses", self.config.url);

        let request = ResponseRequest {
            model: req.model.unwrap_or_else(|| self.config.model.clone()),
            input: req.input,
            instructions: req.instructions,
            temperature: self.config.temperature,
            max_output_tokens: self.config.max_output_tokens,
            tools: req.tools,
            stream: true,
        };

        let initial = self.client.post(&url).json(&request).send().await?;

        if !initial.status().is_success() {
            let status = initial.status();
            let body = initial.text().await.unwrap_or_default();
            return Err(anyhow!("API error {status}: {body}"));
        }

        let mut byte_stream = initial.bytes_stream();
        let mut buffer = String::new();
        let mut tool_call_counter: usize = 0;

        let stream = try_stream! {
            while let Some(chunk) = byte_stream.next().await {
                let chunk = chunk?;
                buffer.push_str(&String::from_utf8_lossy(&chunk));

                while let Some(raw_event) = Self::extract_sse_event(&mut buffer)? {
                    match raw_event {
                        RawStreamEvent::ResponseCreated { response } => {
                            yield StreamEvent::MessageStart {
                                id: response.id.unwrap_or_default(),
                                model: response.model.unwrap_or_default(),
                            };
                        }
                        RawStreamEvent::OutputItemAdded { item, .. } => {
                            if let StreamOutputItem::FunctionCall { call_id, name } = item {
                                let index = tool_call_counter;
                                tool_call_counter += 1;
                                yield StreamEvent::ToolCallStart {
                                    index,
                                    id: call_id,
                                    name,
                                };
                            }
                        }
                        RawStreamEvent::OutputTextDelta { delta, .. } => {
                            if !delta.is_empty() {
                                yield StreamEvent::ContentDelta { text: delta };
                            }
                        }
                        RawStreamEvent::FunctionCallArgumentsDelta { output_index, delta } => {
                            if !delta.is_empty() {
                                yield StreamEvent::ToolCallDelta {
                                    index: output_index,
                                    arguments: delta,
                                };
                            }
                        }
                        RawStreamEvent::ResponseCompleted { response } => {
                            let status = response.status.unwrap_or_else(|| "completed".to_string());
                            yield StreamEvent::Done { finish_reason: status };
                            if let Some(usage) = response.usage {
                                yield StreamEvent::Usage(usage);
                            }
                        }
                        _ => {}
                    }
                }
            }
        };

        Ok(stream)
    }

    fn extract_sse_event(buffer: &mut String) -> OpenAIResult<Option<RawStreamEvent>> {
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
            .map(|obj| obj.insert("type".to_string(), serde_json::Value::String(event_type)));

        match serde_json::from_value::<RawStreamEvent>(json) {
            Ok(event) => Ok(Some(event)),
            Err(e) => Err(OpenAIError::Serialization(e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::pin::pin;
    use super::*;

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

        println!("{:?}",response);

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
        let mut stream = client.chat_stream(req).await.unwrap();

        let mut got_start = false;
        let mut got_content = false;
        let mut got_done = false;

        let mut stream = pin!(stream);

        while let Some(event) = stream.next().await {
            let event = event.unwrap();
            println!("{:?}", event);
            match event {
                StreamEvent::MessageStart { .. } => got_start = true,
                StreamEvent::ContentDelta { .. } => got_content = true,
                StreamEvent::Done { .. } => got_done = true,
                _ => {}
            }
        }

        assert!(got_start, "should receive MessageStart");
        assert!(got_content, "should receive ContentDelta");
        assert!(got_done, "should receive Done");
    }
}
