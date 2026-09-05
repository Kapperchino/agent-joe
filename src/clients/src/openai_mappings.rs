use crate::llm::{ContentBlock, ContentBlockInfo, Delta};
use crate::openai::{ClientRequest, InputItem, OutputItem, Role, StreamEvent, StreamOutputItem};
use crate::{llm, openai};
use tools::tool_defs;
use tools::tool_defs::{ToolDefinition, ToolId};
use tracing::error;

const OPENAI_DEFAULT_SEARCH_TOOL_TYPE: &str = "web_search";

impl TryFrom<llm::ClientRequest> for ClientRequest {
    type Error = anyhow::Error;

    fn try_from(llm_req: llm::ClientRequest) -> anyhow::Result<Self> {
        Ok(ClientRequest {
            input: llm_req
                .messages
                .into_iter()
                .flat_map(|x| {
                    let role = x.role.clone();
                    x.content.into_iter().map(move |c| (c, role.clone()))
                })
                .map(|(content, role)| match content {
                    ContentBlock::MessageBlock { text, phase } => Ok(InputItem::Message {
                        role: role.into(),
                        content: text,
                        phase,
                    }),
                    ContentBlock::ThinkingBlock { .. } => Err(anyhow::anyhow!(
                        "This history contains thinking state incompatible with OpenAI; start a new conversation"
                    )),
                    ContentBlock::OpenAIReasoning(item) => Ok(InputItem::Reasoning(item)),
                    ContentBlock::ToolBlock { tool_id, name, input } => Ok(InputItem::FunctionCall {
                        id: tool_id.id,
                        call_id: tool_id.call_id.ok_or_else(|| anyhow::anyhow!("OpenAI tool call is missing its call_id"))?,
                        name,
                        arguments: serde_json::to_string(&input)?,
                    }),
                    ContentBlock::ToolResult { tool_id, content, .. } => Ok(InputItem::FunctionCallOutput {
                        call_id: tool_id.call_id.ok_or_else(|| anyhow::anyhow!("OpenAI tool result is missing its call_id"))?,
                        output: content,
                    }),
                })
                .collect::<anyhow::Result<Vec<_>>>()?,
            instructions: llm_req.system,
            model: llm_req.model,
            tools: llm_req.tools.into_iter().map(|t| t.into()).collect(),
        })
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
            } => {
                let has_tool_calls = response
                    .output
                    .iter()
                    .any(|x| matches!(x, OutputItem::FunctionCall { .. }));
                Some(llm::StreamEvent::MessageDelta {
                    delta: llm::MessageDeltaContent {
                        stop_reason: Some(if has_tool_calls {
                            llm::StopReason::ToolUse
                        } else {
                            llm::StopReason::EndTurn
                        }),
                    },
                    usage: llm::UsageDelta {
                        output_tokens: response
                            .usage
                            .as_ref()
                            .map(|t| t.output_tokens)
                            .unwrap_or(0),
                        input_tokens: response.usage.as_ref().map(|t| t.input_tokens).unwrap_or(0),
                    },
                })
            }
            StreamEvent::ResponseIncomplete {
                response,
                sequence_number: _,
            } => Some(llm::StreamEvent::Error {
                error: llm::ApiErrorDetail {
                    error_type: "incomplete_response".into(),
                    message: format!(
                        "OpenAI response incomplete: {}",
                        response
                            .incomplete_details
                            .map(|details| details.reason)
                            .unwrap_or_default()
                    ),
                },
            }),
            StreamEvent::ResponseFailed {
                response,
                sequence_number: _,
            } => Some(llm::StreamEvent::Error {
                error: llm::ApiErrorDetail {
                    error_type: "failed_response".into(),
                    message: format!(
                        "OpenAI response failed: {}",
                        response
                            .error
                            .map(|error| error.message)
                            .unwrap_or_default()
                    ),
                },
            }),
            StreamEvent::OutputItemAdded {
                output_index,
                item,
                sequence_number: _,
            } => match item {
                StreamOutputItem::FunctionCall { id, call_id, name } => {
                    Some(llm::StreamEvent::ContentBlockStart {
                        index: output_index,
                        content_block: ContentBlockInfo::ToolUse {
                            id: llm::PendingToolId {
                                call_id: Some(call_id),
                                id,
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
                Some(OutputItem::Reasoning(item)) => Some(llm::StreamEvent::ContentBlockComplete {
                    index: output_index,
                    content: ContentBlock::OpenAIReasoning(item),
                }),
                Some(OutputItem::Message { content, phase, .. }) => {
                    Some(llm::StreamEvent::ContentBlockComplete {
                        index: output_index,
                        content: ContentBlock::MessageBlock {
                            text: content
                                .into_iter()
                                .map(|part| match part {
                                    openai::ContentPart::OutputText { text } => text,
                                })
                                .collect(),
                            phase,
                        },
                    })
                }
                Some(OutputItem::FunctionCall {
                    id,
                    call_id,
                    name,
                    arguments,
                }) => Some(match serde_json::from_str(&arguments) {
                    Ok(input) => llm::StreamEvent::ContentBlockComplete {
                        index: output_index,
                        content: ContentBlock::ToolBlock {
                            tool_id: ToolId {
                                id,
                                call_id: Some(call_id),
                            },
                            name,
                            input,
                        },
                    },
                    Err(err) => llm::StreamEvent::Error {
                        error: llm::ApiErrorDetail {
                            error_type: "invalid_tool_arguments".to_owned(),
                            message: format!("Invalid JSON for tool {name}: {err}"),
                        },
                    },
                }),
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
            StreamEvent::ReasoningTextDone { .. } => Some(llm::StreamEvent::Accum),
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
            StreamEvent::ReasoningSummaryTextDone { .. } => Some(llm::StreamEvent::Accum),
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
            StreamEvent::KeepAlive { .. } => Some(llm::StreamEvent::Accum),
            StreamEvent::WebSearchCallInProgress { .. } => Some(llm::StreamEvent::Accum),
            StreamEvent::WebSearchCallSearching { .. } => Some(llm::StreamEvent::Accum),
            StreamEvent::WebSearchCallCompleted { .. } => Some(llm::StreamEvent::Accum),
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
