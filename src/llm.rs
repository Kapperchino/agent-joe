use crate::claude::ClaudeClient;
use crate::openai::OpenAIClient;
use crate::tool_defs::Tool;
use futures::Stream;
use std::collections::HashMap;

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

impl LLmClient {}

// map from other clients to this
#[derive(Debug, Clone)]
pub struct StreamEvent {}

pub struct ClientRequest {
    messages: Vec<Message>,
    thinking: bool,
    system: Option<String>,
    model: Option<String>,
    // this shit needs to be turned ON
    cache_control: CacheControl,
    tools: Vec<Tool>,
}

pub struct ClientResponse {}

pub struct CacheControl {}

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
