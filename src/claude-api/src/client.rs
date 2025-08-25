use crate::{
    config::ClaudeConfig,
    error::{ClaudeError, Result},
    models::{
        ApiErrorResponse, CreateMessageRequest, CreateMessageResponse, StreamEvent,
        Message, Tool,
    },
};
use reqwest::{header, Client, Response};
use tokio_stream::{Stream, StreamExt};

pub struct ClaudeClient {
    client: Client,
    config: ClaudeConfig,
}

impl ClaudeClient {
    pub fn new(config: ClaudeConfig) -> Result<Self> {
        let mut headers = header::HeaderMap::new();
        headers.insert(
            "x-api-key",
            header::HeaderValue::from_str(&config.api_key)
                .map_err(|_| ClaudeError::InvalidApiKey)?,
        );
        headers.insert(
            "anthropic-version",
            header::HeaderValue::from_str(&config.anthropic_version)
                .map_err(|e| ClaudeError::Configuration(format!("Invalid anthropic version: {}", e)))?,
        );
        headers.insert(
            "content-type",
            header::HeaderValue::from_static("application/json"),
        );

        let client = Client::builder()
            .timeout(config.timeout)
            .default_headers(headers)
            .build()?;

        Ok(Self { client, config })
    }

    pub async fn create_message(&self, request: CreateMessageRequest) -> Result<CreateMessageResponse> {
        let url = self.config.base_url.join("messages")?;
        
        let response = self
            .client
            .post(url)
            .json(&request)
            .send()
            .await?;

        self.handle_response(response).await
    }

    pub async fn stream_message(
        &self,
        mut request: CreateMessageRequest,
    ) -> Result<impl Stream<Item = Result<StreamEvent>>> {
        request.stream = Some(true);
        let url = self.config.base_url.join("messages")?;
        
        let response = self
            .client
            .post(url)
            .json(&request)
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(self.handle_error_response(response).await?);
        }

        let stream = response
            .bytes_stream()
            .map(|chunk| {
                chunk
                    .map_err(ClaudeError::from)
                    .and_then(|bytes| {
                        let text = String::from_utf8_lossy(&bytes);
                        // Parse SSE format
                        for line in text.lines() {
                            if line.starts_with("data: ") {
                                let json_str = &line[6..];
                                if json_str == "[DONE]" {
                                    continue;
                                }
                                return serde_json::from_str::<StreamEvent>(json_str)
                                    .map_err(ClaudeError::from);
                            }
                        }
                        Err(ClaudeError::Configuration(
                            "No valid data line found in stream".to_string()
                        ))
                    })
            });

        Ok(stream)
    }

    async fn handle_response<T>(&self, response: Response) -> Result<T>
    where
        T: serde::de::DeserializeOwned,
    {
        let status = response.status();
        
        if status.is_success() {
            let text = response.text().await?;
            serde_json::from_str(&text).map_err(ClaudeError::from)
        } else {
            Err(self.handle_error_response_with_status(status.as_u16(), response).await?)
        }
    }

    async fn handle_error_response(&self, response: Response) -> Result<ClaudeError> {
        let status = response.status().as_u16();
        self.handle_error_response_with_status(status, response).await
    }

    async fn handle_error_response_with_status(&self, status: u16, response: Response) -> Result<ClaudeError> {
        let error_text = response.text().await.unwrap_or_default();
        
        let error = match serde_json::from_str::<ApiErrorResponse>(&error_text) {
            Ok(api_error) => ClaudeError::ApiError {
                status,
                message: api_error.message,
            },
            Err(_) => match status {
                401 => ClaudeError::Authentication("Invalid API key or unauthorized".to_string()),
                429 => ClaudeError::RateLimit {
                    message: "Rate limit exceeded".to_string(),
                },
                _ => ClaudeError::ApiError {
                    status,
                    message: error_text,
                },
            },
        };

        Ok(error)
    }
}

// Convenience builder for requests
pub struct MessageBuilder {
    model: String,
    max_tokens: u32,
    messages: Vec<Message>,
    system: Option<String>,
    temperature: Option<f32>,
    top_p: Option<f32>,
    top_k: Option<u32>,
    stop_sequences: Option<Vec<String>>,
    tools: Option<Vec<Tool>>,
}

impl MessageBuilder {
    pub fn new(model: impl Into<String>, max_tokens: u32) -> Self {
        Self {
            model: model.into(),
            max_tokens,
            messages: Vec::new(),
            system: None,
            temperature: None,
            top_p: None,
            top_k: None,
            stop_sequences: None,
            tools: None,
        }
    }

    pub fn system(mut self, system: impl Into<String>) -> Self {
        self.system = Some(system.into());
        self
    }

    pub fn add_message(mut self, message: Message) -> Self {
        self.messages.push(message);
        self
    }

    pub fn user_message(mut self, content: impl Into<String>) -> Self {
        self.messages.push(Message::user(content));
        self
    }

    pub fn assistant_message(mut self, content: impl Into<String>) -> Self {
        self.messages.push(Message::assistant(content));
        self
    }

    pub fn temperature(mut self, temperature: f32) -> Self {
        self.temperature = Some(temperature);
        self
    }

    pub fn top_p(mut self, top_p: f32) -> Self {
        self.top_p = Some(top_p);
        self
    }

    pub fn top_k(mut self, top_k: u32) -> Self {
        self.top_k = Some(top_k);
        self
    }

    pub fn stop_sequences(mut self, sequences: Vec<String>) -> Self {
        self.stop_sequences = Some(sequences);
        self
    }

    pub fn tools(mut self, tools: Vec<Tool>) -> Self {
        self.tools = Some(tools);
        self
    }

    pub fn build(self) -> CreateMessageRequest {
        CreateMessageRequest {
            model: self.model,
            max_tokens: self.max_tokens,
            messages: self.messages,
            system: self.system,
            temperature: self.temperature,
            top_p: self.top_p,
            top_k: self.top_k,
            stop_sequences: self.stop_sequences,
            stream: Some(false),
            tools: self.tools,
            metadata: None,
        }
    }
} 