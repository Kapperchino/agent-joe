use crate::llm::{ContentBlock, ContentBlockInfo, Delta};
use crate::openai::{ClientRequest, InputItem, OutputItem, Role, StreamEvent, StreamOutputItem};
use crate::tool_defs::ToolId;
use crate::{llm, openai, tool_defs};

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
                    ContentBlock::ThinkingBlock {
                        thinking,
                        signature,
                        reasoning_id,
                    } => Some(InputItem::Reasoning {
                        id: reasoning_id.unwrap_or_default(),
                        summary: vec![openai::SummaryTextContent {
                            text: thinking,
                            prop_type: "summary_text".to_string(),
                        }],
                    }),
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

impl From<tool_defs::Tool> for openai::Tool {
    fn from(value: tool_defs::Tool) -> Self {
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

        openai::Tool {
            tool_type: "function".to_string(),
            name: name.to_string(),
            description: description.to_string(),
            parameters: openai::FunctionParameters {
                param_type: "object".to_string(),
                properties: properties.into_iter().map(|(k, v)| (k, v.into())).collect(),
                required,
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
                        output_tokens: response.usage.map(|t| t.output_tokens).unwrap_or(0),
                    },
                })
            }
            StreamEvent::OutputItemAdded {
                output_index,
                item,
                sequence_number,
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
                StreamOutputItem::Message { id, role } => {
                    Some(llm::StreamEvent::ContentBlockStart {
                        index: output_index,
                        content_block: ContentBlockInfo::Text {
                            text: "".to_string(),
                        },
                    })
                }
                StreamOutputItem::Reasoning {
                    summary_text_content,
                    content,
                } => Some(llm::StreamEvent::ContentBlockStart {
                    index: output_index,
                    content_block: ContentBlockInfo::Thinking {
                        thinking: "".to_string(),
                    },
                }),
                StreamOutputItem::Unknown => None,
            },
            StreamEvent::OutputItemDone {
                output_index,
                item,
                sequence_number,
            } => Some(llm::StreamEvent::ContentBlockStop {
                index: output_index,
                id: item.map(|item| match item {
                    OutputItem::Message { id, .. } => id,
                    OutputItem::FunctionCall { id, .. } => id,
                    OutputItem::Reasoning { id, .. } => id,
                }),
            }),
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
            _ => None,
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
