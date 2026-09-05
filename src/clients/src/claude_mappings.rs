use crate::claude::{
    CacheControl, ChatResponse, ClientRequest, ContentBlock, ContentBlockInfo, Delta, Message,
    Role, StreamEvent, Tool, ToolSchemaDTO,
};
use crate::llm::ClientResponse;
use crate::{claude, llm};
use tools::tool_defs;
use tools::tool_defs::{ToolDefinition, ToolId};

const CLAUDE_WEB_SEARCH_TOOL_TYPE: &str = "web_search_20250305";
const CLAUDE_WEB_SEARCH_MAX_USES: u32 = 5;

impl From<llm::Role> for claude::Role {
    fn from(value: llm::Role) -> Self {
        match value {
            llm::Role::User => Role::User,
            llm::Role::Assistant => Role::Assistant,
        }
    }
}

impl TryFrom<llm::ContentBlock> for ContentBlock {
    type Error = anyhow::Error;

    fn try_from(value: llm::ContentBlock) -> anyhow::Result<Self> {
        match value {
            llm::ContentBlock::MessageBlock { text, .. } => Ok(ContentBlock::MessageBlock { text }),
            llm::ContentBlock::OpenAIReasoning(_) => Err(anyhow::anyhow!(
                "This history contains OpenAI reasoning state; start a new conversation before using Claude"
            )),
            llm::ContentBlock::ThinkingBlock {
                thinking,
                signature,
                ..
            } => Ok(ContentBlock::ThinkingBlock {
                thinking,
                signature,
            }),
            llm::ContentBlock::ToolBlock {
                tool_id,
                name,
                input,
            } => Ok(ContentBlock::ToolBlock {
                id: tool_id.id,
                name,
                input,
            }),
            llm::ContentBlock::ToolResult {
                tool_id,
                content,
                is_error,
            } => Ok(ContentBlock::ToolResult {
                tool_use_id: tool_id.id,
                content,
                is_error,
            }),
        }
    }
}

impl TryFrom<llm::Message> for Message {
    type Error = anyhow::Error;

    fn try_from(value: llm::Message) -> anyhow::Result<Self> {
        Ok(Message {
            role: value.role.into(),
            content: value
                .content
                .into_iter()
                .map(TryInto::try_into)
                .collect::<anyhow::Result<_>>()?,
        })
    }
}

impl From<&tool_defs::ToolDefinition> for Tool {
    fn from(value: &tool_defs::ToolDefinition) -> Self {
        match value {
            ToolDefinition::Client {
                name,
                description,
                properties,
                required,
            } => Tool::Client {
                name: name.clone(),
                description: description.clone(),
                input_schema: ToolSchemaDTO {
                    name: name.clone(),
                    tool_type: "object".to_string(),
                    properties: properties
                        .clone()
                        .into_iter()
                        .map(|(k, v)| (k, v.into()))
                        .collect(),
                    required: required.clone(),
                },
            },
            ToolDefinition::Search { name } => Tool::Server {
                tool_type: CLAUDE_WEB_SEARCH_TOOL_TYPE.to_string(),
                name: name.clone(),
                max_uses: CLAUDE_WEB_SEARCH_MAX_USES,
            },
        }
    }
}

impl TryFrom<llm::ClientRequest> for ClientRequest {
    type Error = anyhow::Error;

    fn try_from(value: llm::ClientRequest) -> anyhow::Result<Self> {
        let tools: Vec<Tool> = value.tools.iter().map(|t| t.into()).collect();

        Ok(ClientRequest {
            messages: value
                .messages
                .into_iter()
                .map(TryInto::try_into)
                .collect::<anyhow::Result<_>>()?,
            thinking: value.thinking,
            system: value.system,
            model: value.model,
            tools,
            cache_control: CacheControl {
                cache_type: "ephemeral".to_string(),
                ttl: "5m".to_string(),
            },
            effort: None,
        })
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
            } => match content_block {
                ContentBlockInfo::ToolUse { id, name, input } => {
                    llm::StreamEvent::ContentBlockStart {
                        index,
                        content_block: llm::ContentBlockInfo::ToolUse {
                            id: llm::PendingToolId {
                                call_id: None,
                                id: Some(id),
                            },
                            name,
                            input,
                        },
                    }
                }
                ContentBlockInfo::Thinking { thinking } => llm::StreamEvent::ContentBlockStart {
                    index,
                    content_block: llm::ContentBlockInfo::Thinking { thinking },
                },
                ContentBlockInfo::Text { text } => llm::StreamEvent::ContentBlockStart {
                    index,
                    content_block: llm::ContentBlockInfo::Text { text },
                },
                ContentBlockInfo::ServerToolUse { .. }
                | ContentBlockInfo::WebSearchToolResult { .. } => llm::StreamEvent::Accum,
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
            StreamEvent::ContentBlockStop { index } => {
                llm::StreamEvent::ContentBlockStop { index, id: None }
            }
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
                    input_tokens: 0,
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

fn content_block_to_llm(value: ContentBlock) -> Option<llm::ContentBlock> {
    match value {
        ContentBlock::MessageBlock { text } => {
            Some(llm::ContentBlock::MessageBlock { text, phase: None })
        }
        ContentBlock::ThinkingBlock {
            thinking,
            signature,
        } => Some(llm::ContentBlock::ThinkingBlock {
            thinking,
            signature,
            reasoning_id: None,
        }),
        ContentBlock::ToolBlock { id, name, input } => Some(llm::ContentBlock::ToolBlock {
            tool_id: ToolId { call_id: None, id },
            name,
            input,
        }),
        ContentBlock::ToolResult {
            tool_use_id,
            content,
            is_error,
        } => Some(llm::ContentBlock::ToolResult {
            tool_id: ToolId {
                call_id: None,
                id: tool_use_id,
            },
            content,
            is_error,
        }),
        ContentBlock::ServerToolUse { .. } | ContentBlock::WebSearchToolResult { .. } => None,
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
            content: value
                .content
                .into_iter()
                .filter_map(content_block_to_llm)
                .collect(),
            stop_reason: value.stop_reason,
            stop_sequence: value.stop_sequence,
            usage: value.usage,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn maps_search_tool_definition_to_claude_server_tool() {
        let definition = ToolDefinition::Search {
            name: "web_search".to_string(),
        };

        let tool: Tool = (&definition).into();
        let value = serde_json::to_value(tool).unwrap();

        assert_eq!(
            value,
            json!({
                "type": "web_search_20250305",
                "name": "web_search",
                "max_uses": 5
            })
        );
    }
}
