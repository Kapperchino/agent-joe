use crate::claude::{ClaudeClient, Usage};
use crate::config::Config;
use crate::openai::OpenAIClient;
use crate::tool_defs::{Tool, ToolId};
use crate::{ClaudeEffort, OpenAIEffort};
use futures::Stream;
use futures::future::Either;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::str::FromStr;

pub trait LLmClientTrait {
    async fn chat_stream(
        &self,
        req: ClientRequest,
    ) -> Result<impl Stream<Item = anyhow::Result<StreamEvent>> + Send + 'static, anyhow::Error>;
    async fn send_request(&self, request: ClientRequest) -> anyhow::Result<ClientResponse>;
}
pub enum LLmClient {
    Claude {
        client: ClaudeClient,
        config: Config,
    },
    OpenApi {
        client: OpenAIClient,
        config: Config,
    },
}

impl LLmClient {
    pub fn new(config: Config) -> anyhow::Result<LLmClient> {
        match &config {
            Config::Claude(claude_config) => Ok(LLmClient::Claude {
                client: ClaudeClient::new(claude_config.clone())?,
                config,
            }),
            Config::OpenAI(openai_config) => Ok(LLmClient::OpenApi {
                client: OpenAIClient::new(openai_config.clone())?,
                config,
            }),
        }
    }

    pub async fn chat_stream(
        &self,
        req: ClientRequest,
    ) -> Result<impl Stream<Item = anyhow::Result<StreamEvent>> + Send + 'static, anyhow::Error>
    {
        match self {
            LLmClient::Claude { client, .. } => Ok(Either::Left(client.chat_stream(req).await?)),
            LLmClient::OpenApi { client, .. } => Ok(Either::Right(client.chat_stream(req).await?)),
        }
    }

    async fn send_request(&self, request: ClientRequest) -> anyhow::Result<ClientResponse> {
        todo!()
    }

    pub async fn change_model_and_effort(
        &mut self,
        name: String,
        effort: String,
    ) -> anyhow::Result<()> {
        match self.get_config() {
            Config::Claude(config) => {
                config.model = name;
                config.effort = ClaudeEffort::from_str(effort.as_str())?;
            }
            Config::OpenAI(config) => {
                config.model = name;
                config.effort = OpenAIEffort::from_str(effort.as_str())?;
            }
        };
        self.get_config().save().await
    }

    fn get_config(&mut self) -> &mut Config {
        match self {
            LLmClient::Claude { config, .. } => config,
            LLmClient::OpenApi { config, .. } => config,
        }
    }
}

// map from other clients to this
#[derive(Debug, Clone, Serialize, Deserialize)]
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
        // openai specific
        id: Option<String>,
    },
    MessageDelta {
        delta: MessageDeltaContent,
        usage: UsageDelta,
    },
    MessageStop,
    Ping,
    Accum,
    Error {
        error: ApiErrorDetail,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageDeltaContent {
    pub stop_reason: Option<StopReason>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum StopReason {
    EndTurn,
    MaxTokens,
    StopSequence,
    ToolUse,
    Refusal,
    ContextExceeded,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiErrorDetail {
    pub error_type: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageDelta {
    pub output_tokens: u32,
    pub input_tokens: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamMessage {
    pub id: String,
    pub model: String,
    pub role: Role,
    pub usage: StreamUsage,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Delta {
    TextDelta {
        text: String,
    },
    ThinkingDelta {
        thinking: String,
        reasoning_id: Option<String>,
    },
    InputJsonDelta {
        partial_json: String,
    },
    SignatureDelta {
        signature: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ContentBlockInfo {
    ToolUse {
        id: ToolId,
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
        reasoning_id: Option<String>,
    },
    ToolBlock {
        tool_id: ToolId,
        name: String,
        input: HashMap<String, String>,
    },
    ToolResult {
        tool_id: ToolId,
        content: String,
        is_error: Option<bool>,
    },
}

#[derive(Debug, Clone)]
pub struct Message {
    pub role: Role,
    pub content: Vec<ContentBlock>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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
