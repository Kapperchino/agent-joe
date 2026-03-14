use crate::claude::{
    CacheControl, ChatResponse, ClientRequest, ContentBlock, ContentBlockInfo, Delta, Message,
    Role, StreamEvent, Tool, ToolSchemaDTO,
};
use crate::llm::ClientResponse;
use crate::tool_defs::ToolId;
use crate::{claude, llm, tool_defs};

impl From<llm::Role> for claude::Role {
    fn from(value: llm::Role) -> Self {
        match value {
            llm::Role::User => Role::User,
            llm::Role::Assistant => Role::Assistant,
        }
    }
}

impl From<llm::ContentBlock> for ContentBlock {
    fn from(value: llm::ContentBlock) -> Self {
        match value {
            llm::ContentBlock::MessageBlock { text } => ContentBlock::MessageBlock { text },
            llm::ContentBlock::ThinkingBlock {
                thinking,
                signature,
                ..
            } => ContentBlock::ThinkingBlock {
                thinking,
                signature,
            },
            llm::ContentBlock::ToolBlock {
                tool_id,
                name,
                input,
            } => ContentBlock::ToolBlock {
                id: tool_id.id,
                name,
                input,
            },
            llm::ContentBlock::ToolResult {
                tool_id,
                content,
                is_error,
            } => ContentBlock::ToolResult {
                tool_use_id: tool_id.id,
                content,
                is_error,
            },
        }
    }
}

impl From<llm::Message> for Message {
    fn from(value: llm::Message) -> Self {
        Message {
            role: value.role.into(),
            content: value.content.into_iter().map(|c| c.into()).collect(),
        }
    }
}

impl From<&tool_defs::Tool> for Tool {
    fn from(value: &tool_defs::Tool) -> Self {
        use tool_defs::ToolDefTrait;
        macro_rules! extract {
            ($variant:ident) => {{
                (
                    tool_defs::$variant::tool_name(),
                    tool_defs::$variant::tool_description(),
                    tool_defs::$variant::field_properties(),
                    tool_defs::$variant::required_fields(),
                )
            }};
        }

        let (name, description, properties, required) = match value {
            tool_defs::Tool::ReadFile(_) => extract!(ReadFile),
            tool_defs::Tool::InsertAfterLine(_) => extract!(InsertAfterLine),
            tool_defs::Tool::StringReplace(_) => extract!(StringReplace),
            tool_defs::Tool::CargoCheck(_) => extract!(CargoCheck),
        };

        Tool {
            name: name.to_string(),
            description: description.to_string(),
            input_schema: ToolSchemaDTO {
                name: name.to_string(),
                tool_type: "object".to_string(),
                properties: properties.into_iter().map(|(k, v)| (k, v.into())).collect(),
                required,
            },
        }
    }
}

impl From<llm::ClientRequest> for ClientRequest {
    fn from(value: llm::ClientRequest) -> Self {
        let tools: Vec<Tool> = value.tools.iter().map(|t| t.into()).collect();

        ClientRequest {
            messages: value.messages.into_iter().map(|m| m.into()).collect(),
            thinking: value.thinking,
            system: value.system,
            model: value.model,
            tools,
            cache_control: CacheControl {
                cache_type: "ephemeral".to_string(),
                ttl: "5m".to_string(),
            },
        }
    }
}

impl Into<llm::StreamEvent> for StreamEvent {
    fn into(self) -> llm::StreamEvent {
        match self {
            StreamEvent::MessageStart { message } => llm::StreamEvent::MessageStart {
                message: llm::StreamMessage {
                    id: message.id,
                    model: message.model,
                    role: match message.role {
                        Role::User => llm::Role::User,
                        Role::Assistant => llm::Role::Assistant,
                    },
                    usage: llm::StreamUsage {
                        input_tokens: message.usage.input_tokens,
                        cache_creation_input_tokens: message.usage.cache_creation_input_tokens,
                        cache_read_input_tokens: message.usage.cache_read_input_tokens,
                        output_tokens: message.usage.output_tokens,
                    },
                },
            },
            StreamEvent::ContentBlockStart {
                index,
                content_block,
            } => llm::StreamEvent::ContentBlockStart {
                index,
                content_block: match content_block {
                    ContentBlockInfo::ToolUse { id, name, input } => {
                        llm::ContentBlockInfo::ToolUse {
                            id: ToolId { call_id: None, id },
                            name,
                            input,
                        }
                    }
                    ContentBlockInfo::Thinking { thinking } => {
                        llm::ContentBlockInfo::Thinking { thinking }
                    }
                    ContentBlockInfo::Text { text } => llm::ContentBlockInfo::Text { text },
                },
            },
            StreamEvent::ContentBlockDelta { index, delta } => {
                llm::StreamEvent::ContentBlockDelta {
                    index,
                    delta: match delta {
                        Delta::TextDelta { text } => llm::Delta::TextDelta { text },
                        Delta::ThinkingDelta { thinking } => llm::Delta::ThinkingDelta {
                            thinking,
                            reasoning_id: None,
                        },
                        Delta::InputJsonDelta { partial_json } => {
                            llm::Delta::InputJsonDelta { partial_json }
                        }
                        Delta::SignatureDelta { signature } => {
                            llm::Delta::SignatureDelta { signature }
                        }
                    },
                }
            }
            StreamEvent::ContentBlockStop { index } => llm::StreamEvent::ContentBlockStop { index, id: None },
            StreamEvent::MessageDelta { delta, usage } => llm::StreamEvent::MessageDelta {
                delta: llm::MessageDeltaContent {
                    stop_reason: delta.stop_reason.and_then(|s| match s.as_str() {
                        "end_turn" => Some(llm::StopReason::EndTurn),
                        "max_tokens" => Some(llm::StopReason::MaxTokens),
                        "stop_sequence" => Some(llm::StopReason::StopSequence),
                        "tool_use" => Some(llm::StopReason::ToolUse),
                        "refusal" => Some(llm::StopReason::Refusal),
                        "context_exceeded" => Some(llm::StopReason::ContextExceeded),
                        _ => None,
                    }),
                },
                usage: llm::UsageDelta {
                    output_tokens: usage.output_tokens,
                },
            },
            StreamEvent::MessageStop => llm::StreamEvent::MessageStop,
            StreamEvent::Ping => llm::StreamEvent::Ping,
            StreamEvent::Error { error } => llm::StreamEvent::Error {
                error: llm::ApiErrorDetail {
                    error_type: error.error_type,
                    message: error.message,
                },
            },
        }
    }
}

impl From<Role> for llm::Role {
    fn from(value: Role) -> Self {
        match value {
            Role::User => llm::Role::User,
            Role::Assistant => llm::Role::Assistant,
        }
    }
}

impl From<ContentBlock> for llm::ContentBlock {
    fn from(value: ContentBlock) -> Self {
        match value {
            ContentBlock::MessageBlock { text } => llm::ContentBlock::MessageBlock { text },
            ContentBlock::ThinkingBlock {
                thinking,
                signature,
            } => llm::ContentBlock::ThinkingBlock {
                thinking,
                signature,
                reasoning_id: None,
            },
            ContentBlock::ToolBlock { id, name, input } => llm::ContentBlock::ToolBlock {
                tool_id: ToolId { call_id: None, id },
                name,
                input,
            },
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => llm::ContentBlock::ToolResult {
                tool_id: ToolId {
                    call_id: None,
                    id: tool_use_id,
                },
                content,
                is_error,
            },
        }
    }
}

impl From<tool_defs::ToolProperty> for claude::ToolProperty {
    fn from(value: tool_defs::ToolProperty) -> Self {
        match value {
            tool_defs::ToolProperty::Value {
                name,
                prop_type,
                description,
            } => claude::ToolProperty::Value {
                name,
                prop_type,
                description,
            },
            tool_defs::ToolProperty::Object {
                name,
                prop_type,
                description,
                properties,
            } => claude::ToolProperty::Object {
                name,
                prop_type,
                description,
                properties: properties.into_iter().map(|(k, v)| (k, v.into())).collect(),
            },
        }
    }
}

impl From<ChatResponse> for ClientResponse {
    fn from(value: ChatResponse) -> Self {
        ClientResponse {
            id: value.id,
            model: value.model,
            res_type: value.res_type,
            role: value.role.into(),
            content: value.content.into_iter().map(|c| c.into()).collect(),
            stop_reason: value.stop_reason,
            stop_sequence: value.stop_sequence,
            usage: value.usage,
        }
    }
}
