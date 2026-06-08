use crate::llm::{ContentBlock, ContentBlockInfo, Delta};
use crate::openai::{ClientRequest, InputItem, OutputItem, Role, StreamEvent, StreamOutputItem};
use crate::{llm, openai};
use tools::tool_defs;
use tools::tool_defs::{ToolDefinition, ToolId};
use tracing::error;

const OPENAI_DEFAULT_SEARCH_TOOL_TYPE: &str = "web_search";

impl From<llm::ClientRequest> for ClientRequest {
    fn from(llm_req: llm::ClientRequest) -> Self {
        ClientRequest {
            input: llm_req
                .messages
                .into_iter()
                .flat_map(|x| {
                    let role = x.role.clone();
                    x.content.into_iter().map(move |c| (c, role.clone()))
                })
                .filter_map(|(content, role)| match content {
                    ContentBlock::MessageBlock { text } => Some(InputItem::Message {
                        role: role.into(),
                        content: text,
                    }),
                    ContentBlock::ThinkingBlock { .. } => None,
                    ContentBlock::ToolBlock {
                        tool_id,
                        name,
                        input,
                    } => Some(InputItem::FunctionCall {
                        id: tool_id.id,
                        call_id: tool_id.call_id.unwrap_or_default(),
                        name,
                        arguments: serde_json::to_string(&input).unwrap_or_default(),
                    }),
                    ContentBlock::ToolResult {
                        tool_id, content, ..
                    } => Some(InputItem::FunctionCallOutput {
                        call_id: tool_id.call_id.unwrap_or_default(),
                        output: content,
                    }),
                })
                .collect(),
            instructions: llm_req.system,
            model: llm_req.model,
            tools: llm_req.tools.into_iter().map(|t| t.into()).collect(),
        }
    }
}

impl From<llm::Role> for Role {
    fn from(value: llm::Role) -> Self {
        match value {
            llm::Role::User => Role::User,
            llm::Role::Assistant => Role::Assistant,
        }
    }
}

impl From<tool_defs::ToolDefinition> for openai::Tool {
    fn from(value: tool_defs::ToolDefinition) -> Self {
        match value {
            ToolDefinition::Client {
                name,
                description,
                properties,
                required,
            } => openai::Tool::Function {
                tool_type: "function".to_string(),
                name,
                description,
                parameters: openai::FunctionParameters {
                    param_type: "object".to_string(),
                    properties: properties.into_iter().map(|(k, v)| (k, v.into())).collect(),
                    required,
                },
            },
            ToolDefinition::Search { name } => openai::Tool::WebSearch {
                tool_type: if name.is_empty() {
                    OPENAI_DEFAULT_SEARCH_TOOL_TYPE.to_string()
                } else {
                    name
                },
            },
        }
    }
}

impl From<StreamEvent> for Option<llm::StreamEvent> {
    fn from(event: StreamEvent) -> Self {
        match event {
            StreamEvent::ResponseCreated {
                response,
                sequence_number: _,
            } => Some(llm::StreamEvent::MessageStart {
                message: llm::StreamMessage {
                    id: response.id,
                    model: response.model,
                    role: llm::Role::Assistant,
                    usage: Default::default(),
                },
            }),
            StreamEvent::ResponseCompleted {
                response,
                sequence_number: _,
            }
            | StreamEvent::ResponseIncomplete {
                response,
                sequence_number: _,
            }
            | StreamEvent::ResponseFailed {
                response,
                sequence_number: _,
            } => {
                let has_tool_calls = response
                    .output
                    .iter()
                    .any(|x| matches!(x, OutputItem::FunctionCall { .. }));
                let stop_reason = if has_tool_calls {
                    Some(llm::StopReason::ToolUse)
                } else {
                    match response.status.as_deref() {
                        Some("completed") => Some(llm::StopReason::EndTurn),
                        Some("incomplete") => Some(llm::StopReason::MaxTokens),
                        Some("failed") => Some(llm::StopReason::ContextExceeded),
                        _ => None,
                    }
                };
                Some(llm::StreamEvent::MessageDelta {
                    delta: llm::MessageDeltaContent { stop_reason },
                    usage: llm::UsageDelta {
                        output_tokens: response.usage.clone().map(|t| t.output_tokens).unwrap_or(0),
                        input_tokens: response.usage.map(|t| t.input_tokens).unwrap_or(0),
                    },
                })
            }
            StreamEvent::OutputItemAdded {
                output_index,
                item,
                sequence_number: _,
            } => match item {
                StreamOutputItem::FunctionCall { id, call_id, name } => {
                    Some(llm::StreamEvent::ContentBlockStart {
                        index: output_index,
                        content_block: ContentBlockInfo::ToolUse {
                            id: ToolId {
                                call_id: Some(call_id),
                                id: id.unwrap_or_default(),
                            },
                            name,
                            input: Default::default(),
                        },
                    })
                }
                StreamOutputItem::Message { .. } => Some(llm::StreamEvent::ContentBlockStart {
                    index: output_index,
                    content_block: ContentBlockInfo::Text {
                        text: "".to_string(),
                    },
                }),
                StreamOutputItem::Reasoning { .. } => Some(llm::StreamEvent::ContentBlockStart {
                    index: output_index,
                    content_block: ContentBlockInfo::Thinking {
                        thinking: "".to_string(),
                    },
                }),
                StreamOutputItem::WebSearchCall { .. } => Some(llm::StreamEvent::Accum),
            },
            StreamEvent::OutputItemDone {
                output_index,
                item,
                sequence_number: _,
            } => match item {
                Some(OutputItem::Message { id, .. })
                | Some(OutputItem::FunctionCall { id, .. })
                | Some(OutputItem::Reasoning { id, .. }) => {
                    Some(llm::StreamEvent::ContentBlockStop {
                        index: output_index,
                        id: Some(id),
                    })
                }
                Some(OutputItem::WebSearchCall { .. }) => Some(llm::StreamEvent::Accum),
                None => Some(llm::StreamEvent::ContentBlockStop {
                    index: output_index,
                    id: None,
                }),
            },
            StreamEvent::OutputTextDelta {
                output_index,
                delta,
                ..
            } => Some(llm::StreamEvent::ContentBlockDelta {
                index: output_index,
                delta: Delta::TextDelta { text: delta },
            }),
            StreamEvent::FunctionCallArgumentsDelta {
                output_index,
                delta,
                ..
            } => Some(llm::StreamEvent::ContentBlockDelta {
                index: output_index,
                delta: Delta::InputJsonDelta {
                    partial_json: delta,
                },
            }),
            // StreamEvent::FunctionCallArgumentsDone { output_index, .. } => {
            //     Some(llm::StreamEvent::ContentBlockStop {
            //         index: output_index,
            //     })
            // }
            StreamEvent::ReasoningTextDelta {
                item_id,
                output_index,
                delta,
                ..
            } => Some(llm::StreamEvent::ContentBlockDelta {
                index: output_index,
                delta: Delta::ThinkingDelta {
                    thinking: delta.to_string(),
                    reasoning_id: Some(item_id),
                },
            }),
            StreamEvent::ReasoningTextDone { output_index, .. } => {
                Some(llm::StreamEvent::Accum)
            }
            StreamEvent::ReasoningSummaryTextDelta {
                item_id,
                output_index,
                delta,
                ..
            } => Some(llm::StreamEvent::ContentBlockDelta {
                index: output_index,
                delta: Delta::ThinkingDelta {
                    thinking: delta.to_string(),
                    reasoning_id: Some(item_id),
                },
            }),
            StreamEvent::ReasoningSummaryTextDone { output_index, .. } => {
                Some(llm::StreamEvent::ContentBlockStop {
                    index: output_index,
                    id: None,
                })
            }
            StreamEvent::Error {
                code,
                message,
                sequence_number: _,
            } => Some(llm::StreamEvent::Error {
                error: llm::ApiErrorDetail {
                    error_type: code,
                    message,
                },
            }),
            StreamEvent::ResponseQueued { .. } => Some(llm::StreamEvent::Accum),
            StreamEvent::ResponseInProgress { .. } => Some(llm::StreamEvent::Accum),
            StreamEvent::ContentPartAdded { .. } => Some(llm::StreamEvent::Accum),
            StreamEvent::ContentPartDone { .. } => Some(llm::StreamEvent::Accum),
            StreamEvent::OutputTextDone { .. } => Some(llm::StreamEvent::Accum),
            StreamEvent::OutputTextAnnotationAdded { .. } => Some(llm::StreamEvent::Accum),
            StreamEvent::FunctionCallArgumentsDone { .. } => Some(llm::StreamEvent::Accum),
            StreamEvent::ReasoningSummaryPartAdded { .. } => Some(llm::StreamEvent::Accum),
            StreamEvent::ReasoningSummaryPartDone { .. } => Some(llm::StreamEvent::Accum),
            StreamEvent::RefusalDelta { .. } => None,
            StreamEvent::RefusalDone { refusal, .. } => {
                error!("{refusal}");
                None
            }
        }
    }
}

impl From<tool_defs::ToolProperty> for openai::ToolProperty {
    fn from(value: tool_defs::ToolProperty) -> Self {
        match value {
            tool_defs::ToolProperty::Value {
                name,
                prop_type,
                description,
            } => openai::ToolProperty::Value {
                name,
                prop_type,
                description,
            },
            tool_defs::ToolProperty::Object {
                name,
                prop_type,
                description,
                properties,
            } => openai::ToolProperty::Object {
                name,
                prop_type,
                description,
                properties: properties.into_iter().map(|(k, v)| (k, v.into())).collect(),
            },
        }
    }
}
