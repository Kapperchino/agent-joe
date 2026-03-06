use crate::llm::{ContentBlock, ContentBlockInfo, Delta};
use crate::openai::{ClientRequest, InputItem, Role, StreamEvent, StreamOutputItem};
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
                .map(|(content, role)| {
                    match content {
                        ContentBlock::MessageBlock { text } => InputItem::Message {
                            role: role.into(),
                            content: text,
                        },
                        ContentBlock::ThinkingBlock { thinking, .. } => InputItem::Message {
                            role: role.into(),
                            content: thinking,
                        },
                        ContentBlock::ToolBlock { id, name, input } => {
                            // doesn't matter here
                            InputItem::Message {
                                role: role.into(),
                                content: name,
                            }
                        }
                        ContentBlock::ToolResult {
                            tool_use_id,
                            content,
                            ..
                        } => InputItem::FunctionCallOutput {
                            call_id: tool_use_id,
                            output: content,
                        },
                    }
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
            } => Some(llm::StreamEvent::MessageDelta {
                delta: llm::MessageDeltaContent {
                    stop_reason: response.status,
                },
                usage: llm::UsageDelta {
                    output_tokens: response.usage.map(|t| t.output_tokens).unwrap_or(0),
                },
            }),
            StreamEvent::OutputItemAdded {
                output_index,
                item,
                sequence_number,
            } => match item {
                StreamOutputItem::FunctionCall { id, call_id, name } => {
                    Some(llm::StreamEvent::ContentBlockStart {
                        index: output_index,
                        content_block: ContentBlockInfo::ToolUse {
                            id: call_id,
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
                StreamOutputItem::Unknown => None,
            },
            StreamEvent::ContentPartDone { output_index, .. } => {
                Some(llm::StreamEvent::ContentBlockStop {
                    index: output_index,
                })
            }
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
            StreamEvent::FunctionCallArgumentsDone { output_index, .. } => {
                Some(llm::StreamEvent::ContentBlockStop {
                    index: output_index,
                })
            }
            StreamEvent::ReasoningSummaryTextDelta {
                output_index,
                delta,
                ..
            } => Some(llm::StreamEvent::ContentBlockDelta {
                index: output_index,
                delta: Delta::ThinkingDelta {
                    thinking: delta.to_string(),
                },
            }),
            StreamEvent::ReasoningSummaryTextDone { output_index, .. } => {
                Some(llm::StreamEvent::ContentBlockStop {
                    index: output_index,
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
