use crate::claude::{ClaudeClient, Usage};
use crate::openai::OpenAIClient;
use crate::tool_defs::Tool;
use futures::Stream;
use futures::future::Either;
use std::collections::HashMap;

pub trait LLmClientTrait {
    async fn chat_stream(
        &self,
        req: ClientRequest,
    ) -> Result<impl Stream<Item = anyhow::Result<StreamEvent>> + Send + 'static, anyhow::Error>;
    async fn send_request(&self, request: ClientRequest) -> anyhow::Result<ClientResponse>;
}
pub enum LLmClient {
    Claude(ClaudeClient),
    OpenApi(OpenAIClient),
}

impl LLmClient {
    pub async fn chat_stream(
        &self,
        req: ClientRequest,
    ) -> Result<impl Stream<Item = anyhow::Result<StreamEvent>> + Send + 'static, anyhow::Error>
    {
        match self {
            LLmClient::Claude(claude) => Ok(Either::Left(claude.chat_stream(req).await?)),
            LLmClient::OpenApi(openai) => Ok(Either::Right(openai.chat_stream(req).await?)),
        }
    }

    async fn send_request(&self, request: ClientRequest) -> anyhow::Result<ClientResponse> {
        todo!()
    }
}

// map from other clients to this
#[derive(Debug, Clone)]
pub enum StreamEvent {
    MessageStart {
        message: StreamMessage,
    },
    ContentBlockStart {
        index: usize,
        content_block: ContentBlockInfo,
    },
    ContentBlockDelta {
        index: usize,
        delta: Delta,
    },
    ContentBlockStop {
        index: usize,
    },
    MessageDelta {
        delta: MessageDeltaContent,
        usage: UsageDelta,
    },
    MessageStop,
    Ping,
    Error {
        error: ApiErrorDetail,
    },
}

#[derive(Debug, Clone)]
pub struct MessageDeltaContent {
    pub stop_reason: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ApiErrorDetail {
    pub error_type: String,
    pub message: String,
}

#[derive(Debug, Clone)]
pub struct UsageDelta {
    pub output_tokens: u32,
}

#[derive(Debug, Clone)]
pub struct StreamMessage {
    pub id: String,
    pub model: String,
    pub role: Role,
    pub usage: StreamUsage,
}

#[derive(Debug, Clone, Default)]
pub struct StreamUsage {
    pub input_tokens: u32,
    pub cache_creation_input_tokens: u32,
    pub cache_read_input_tokens: u32,
    pub output_tokens: u32,
}

pub struct ClientRequest {
    pub messages: Vec<Message>,
    pub thinking: bool,
    pub system: Option<String>,
    pub model: Option<String>,
    pub tools: Vec<Tool>,
}

impl ClientRequest {
    pub fn new(messages: Vec<Message>) -> ClientRequest {
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

#[derive(Debug)]
pub struct ClientResponse {
    pub id: String,
    pub model: String,
    pub res_type: String,
    pub role: Role,
    pub content: Vec<ContentBlock>,
    pub stop_reason: Option<String>,
    pub stop_sequence: Option<String>,
    pub usage: Usage,
}

pub struct CacheControl {}

#[derive(Debug, Clone)]
pub enum Delta {
    TextDelta { text: String },
    ThinkingDelta { thinking: String },
    InputJsonDelta { partial_json: String },
    SignatureDelta { signature: String },
}

#[derive(Debug, Clone)]
pub enum ContentBlockInfo {
    ToolUse {
        id: String,
        name: String,
        input: HashMap<String, String>,
    },
    Thinking {
        thinking: String,
    },
    Text {
        text: String,
    },
}

#[derive(Debug, Clone)]
pub enum ContentBlock {
    MessageBlock {
        text: String,
    },
    ThinkingBlock {
        thinking: String,
        signature: String,
    },
    ToolBlock {
        id: String,
        name: String,
        input: HashMap<String, String>,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
        is_error: Option<bool>,
    },
}

#[derive(Debug, Clone)]
pub struct Message {
    pub role: Role,
    pub content: Vec<ContentBlock>,
}

#[derive(Debug, Clone)]
pub enum Role {
    User,
    Assistant,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::openai::OpenAIConfig;
    use futures::StreamExt;
    use std::pin::pin;

    #[tokio::test]
    async fn test_llm_client_openai_chat_stream() {
        let api_key = std::env::var("OPENAI_KEY").expect("OPENAI_KEY must be set");
        let config = OpenAIConfig {
            api_key,
            ..Default::default()
        };
        let openai = OpenAIClient::new(config).unwrap();
        let client = LLmClient::OpenApi(openai);

        let req = ClientRequest::new(vec![Message::new("Say hello".to_string())]);
        let stream = client.chat_stream(req).await.unwrap();
        let mut stream = pin!(stream);

        let mut event_count = 0;
        while let Some(event) = stream.next().await {
            let event = event.unwrap();
            event_count += 1;
            println!("{:?}", event);
        }

        assert!(event_count > 0, "should receive at least one stream event");
    }
}
