use crate::claude::{ClaudeClient, Usage};
use crate::config::{Config, ConfigContext};
use crate::openai::OpenAIClient;
use crate::{ClaudeEffort, OpenAIEffort};
use futures::Stream;
use futures::future::Either;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fmt::{Display, Formatter};
use std::str::FromStr;
use tools::tool_defs::{NonEmptyString, ToolDefinition, ToolId};

pub trait LLmClientTrait {
    async fn chat_stream(
        &self,
        req: ClientRequest,
    ) -> Result<impl Stream<Item = anyhow::Result<StreamEvent>> + Send + 'static, anyhow::Error>;
    async fn send_request(&self, request: ClientRequest) -> anyhow::Result<ClientResponse>;
}

#[derive(Clone)]
pub enum LLmClient {
    Claude {
        client: ClaudeClient,
        config: ConfigContext,
    },
    OpenApi {
        client: OpenAIClient,
        config: ConfigContext,
    },
}

impl LLmClient {
    pub fn new(config_context: ConfigContext) -> anyhow::Result<LLmClient> {
        let config = config_context.get_config();
        match &config {
            Config::Claude(claude_config) => Ok(LLmClient::Claude {
                client: ClaudeClient::new(claude_config.clone())?,
                config: config_context.clone(),
            }),
            Config::OpenAI(openai_config) => Ok(LLmClient::OpenApi {
                client: OpenAIClient::new(openai_config.clone())?,
                config: config_context.clone(),
            }),
        }
    }

    pub async fn chat_stream(
        &mut self,
        req: ClientRequest,
    ) -> Result<impl Stream<Item = anyhow::Result<StreamEvent>> + Send + 'static, anyhow::Error>
    {
        self.refresh_config()?;
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
        let mut conf = self.get_config();
        match &mut conf {
            Config::Claude(config) => {
                config.model = name;
                config.effort = ClaudeEffort::from_str(effort.as_str())?;
            }
            Config::OpenAI(config) => {
                config.model = name;
                config.effort = OpenAIEffort::from_str(effort.as_str())?;
            }
        };

        self.save_config(conf).await
    }

    fn get_config(&self) -> Config {
        match self {
            LLmClient::Claude { config, .. } | LLmClient::OpenApi { config, .. } => {
                config.get_config()
            }
        }
    }

    fn refresh_config(&mut self) -> anyhow::Result<()> {
        let current = self.get_config();
        match (self, current) {
            (LLmClient::Claude { client, .. }, Config::Claude(config)) => {
                if client.config.auth != config.auth {
                    *client = ClaudeClient::new(config)?;
                } else {
                    client.config = config;
                }
                Ok(())
            }
            (LLmClient::OpenApi { client, .. }, Config::OpenAI(config)) => {
                if client.config.auth != config.auth {
                    *client = OpenAIClient::new(config)?;
                } else {
                    client.config = config;
                }
                Ok(())
            }
            _ => Err(anyhow::anyhow!("Changing providers requires a new session")),
        }
    }
    async fn save_config(&mut self, config: Config) -> anyhow::Result<()> {
        match self {
            LLmClient::Claude {
                config: context, ..
            } => context.update_config(config),
            LLmClient::OpenApi {
                config: context, ..
            } => context.update_config(config),
        }
        .await
    }
}

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
        id: Option<NonEmptyString>,
    },
    ContentBlockComplete {
        index: usize,
        content: ContentBlock,
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
    pub tools: Vec<ToolDefinition>,
}

impl ClientRequest {
    pub fn with_system(mut self, instructions: String) -> Self {
        self.system = Some(instructions);
        self
    }

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

    pub fn with_tools(self, tools: Vec<ToolDefinition>) -> ClientRequest {
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
pub struct PendingToolId {
    pub id: Option<NonEmptyString>,
    pub call_id: Option<NonEmptyString>,
}

impl PendingToolId {
    pub fn complete(&self, id: Option<NonEmptyString>) -> anyhow::Result<ToolId> {
        Ok(ToolId {
            id: id
                .or_else(|| self.id.clone())
                .ok_or_else(|| anyhow::anyhow!("Completed tool call is missing its ID"))?,
            call_id: self.call_id.clone(),
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ContentBlockInfo {
    ToolUse {
        id: PendingToolId,
        name: NonEmptyString,
        input: serde_json::Map<String, Value>,
    },
    Thinking {
        thinking: String,
    },
    Text {
        text: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessagePhase {
    Commentary,
    FinalAnswer,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ContentBlock {
    MessageBlock {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        phase: Option<MessagePhase>,
    },
    ThinkingBlock {
        thinking: String,
        signature: String,
        reasoning_id: Option<String>,
    },
    OpenAIReasoning(crate::openai::ReasoningItem),
    ToolBlock {
        tool_id: ToolId,
        name: NonEmptyString,
        input: serde_json::Map<String, Value>,
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

impl Display for Message {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        for block in &self.content {
            match block {
                ContentBlock::MessageBlock { text, .. } => {
                    writeln!(f, "{text}")?;
                }
                ContentBlock::ThinkingBlock {
                    thinking,
                    signature: _,
                    reasoning_id,
                } => {
                    match reasoning_id {
                        Some(id) => writeln!(f, "[thinking:{id}]")?,
                        None => writeln!(f, "[thinking]")?,
                    }
                    writeln!(f, "{thinking}")?;
                }
                ContentBlock::ToolBlock {
                    tool_id,
                    name,
                    input,
                } => {
                    writeln!(f, "[tool:{}:{}]", name, tool_id.id)?;
                    writeln!(f, "{}", Value::Object(input.clone()))?;
                }
                ContentBlock::OpenAIReasoning(item) => {
                    writeln!(f, "[thinking:{}]", item.id)?;
                    for part in &item.summary {
                        writeln!(f, "{}", part.text)?;
                    }
                }
                ContentBlock::ToolResult {
                    tool_id,
                    content,
                    is_error,
                } => {
                    match is_error {
                        Some(true) => writeln!(f, "[tool_result:{}:error]", tool_id.id)?,
                        _ => writeln!(f, "[tool_result:{}]", tool_id.id)?,
                    }
                    writeln!(f, "{content}")?;
                }
            }
        }

        Ok(())
    }
}

impl Message {
    pub fn text(&self) -> String {
        self.content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::MessageBlock { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn new(message: String) -> Self {
        Message {
            role: Role::User,
            content: vec![ContentBlock::MessageBlock {
                text: message,
                phase: None,
            }],
        }
    }

    pub fn new_assistant(message: String) -> Self {
        Message {
            role: Role::Assistant,
            content: vec![ContentBlock::MessageBlock {
                text: message,
                phase: None,
            }],
        }
    }
}
