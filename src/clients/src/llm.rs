use futures::Stream;
use std::collections::HashMap;
use crate::claude::ClaudeClient;
use crate::openai::OpenAIClient;

trait LLmClientTrait {
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
        todo!();
        #[allow(unreachable_code)]
        Ok(futures::stream::empty())
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
    messages: Vec<Message>,
    thinking: bool,
    system: Option<String>,
    model: Option<String>,
    tools: Vec<Tool>,
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

pub struct ClientResponse {}

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
