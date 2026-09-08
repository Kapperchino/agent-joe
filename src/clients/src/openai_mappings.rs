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
                .flat_map(|message| {
                    message
                        .content
                        .into_iter()
                        .map(move |content| input_items(content, message.role.clone()))
                })
                .try_fold(Vec::new(), |mut items, next| {
                    items.extend(next?);
                    Ok::<_, anyhow::Error>(items)
                })?,
            max_output_tokens: llm_req.max_output_tokens,
            instructions: llm_req.system,
            model: llm_req.model,
            tools: llm_req.tools.into_iter().map(|t| t.into()).collect(),
        })
    }
}

fn input_items(content: ContentBlock, role: llm::Role) -> anyhow::Result<Vec<InputItem>> {
    match content {
        ContentBlock::MessageBlock { text, phase } => Ok(vec![InputItem::Message {
            role: role.into(),
            content: text,
            phase,
        }]),
        ContentBlock::ThinkingBlock { .. } => Err(anyhow::anyhow!(
            "This history contains thinking state incompatible with OpenAI; start a new conversation"
        )),
        ContentBlock::OpenAIReasoning(item) => Ok(vec![InputItem::Reasoning(item)]),
        ContentBlock::OpenAICompaction(window) => Ok(Vec::from(window)
            .into_iter()
            .map(InputItem::Native)
            .collect()),
        ContentBlock::ToolBlock {
            tool_id,
            name,
            input,
        } => Ok(vec![InputItem::FunctionCall {
            id: tool_id.id,
            call_id: tool_id
                .call_id
                .ok_or_else(|| anyhow::anyhow!("OpenAI tool call is missing its call_id"))?,
            name,
            arguments: serde_json::to_string(&input)?,
        }]),
        ContentBlock::ToolResult {
            tool_id, content, ..
        } => Ok(vec![InputItem::FunctionCallOutput {
            call_id: tool_id
                .call_id
                .ok_or_else(|| anyhow::anyhow!("OpenAI tool result is missing its call_id"))?,
            output: content,
        }]),
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
            } => {
                let reason = response
                    .incomplete_details
                    .map(|details| details.reason)
                    .unwrap_or_default();
                let code = match reason.as_str() {
                    "context_length_exceeded" | "context_window_exceeded" | "context_exceeded" => {
                        "context_length_exceeded"
                    }
                    "content_filter" => "content_filter",
                    _ => "incomplete_response",
                };
                Some(llm::StreamEvent::Error {
                    error: llm::ApiErrorDetail {
                        error_type: code.into(),
                        message: format!("OpenAI response incomplete: {reason}"),
                    },
                })
            }
            StreamEvent::ResponseFailed {
                response,
                sequence_number: _,
            } => Some(llm::StreamEvent::Error {
                error: llm::ApiErrorDetail {
                    error_type: response
                        .error
                        .as_ref()
                        .and_then(|error| error.code.clone())
                        .unwrap_or_else(|| "failed_response".into()),
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
            tool_defs::ToolProperty::Schema(schema) => openai::ToolProperty::Schema(schema),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::failure::{Failure, FailureKind};

    #[test]
    fn cargo_selection_schemas_survive_both_provider_mappings() {
        use tools::tool_defs::ToolDefTrait;
        let properties = <tools::cargo_tools::Cargo as ToolDefTrait>::field_properties();
        for name in [
            "operation",
            "target",
            "args",
            "environment",
            "process_id",
            "offsets",
        ] {
            let property = properties[name].clone();
            let expected = match &property {
                tools::tool_defs::ToolProperty::Schema(schema) => schema.clone(),
                _ => panic!("Expected a structured schema"),
            };
            let openai: crate::openai::ToolProperty = property.clone().into();
            let claude: crate::claude::ToolProperty = property.into();
            assert_eq!(serde_json::to_value(&openai).unwrap(), expected);
            assert_eq!(serde_json::to_value(&claude).unwrap(), expected);
            assert_eq!(
                serde_json::to_value(
                    serde_json::from_value::<crate::openai::ToolProperty>(expected.clone())
                        .unwrap()
                )
                .unwrap(),
                expected
            );
            assert_eq!(
                serde_json::to_value(
                    serde_json::from_value::<crate::claude::ToolProperty>(expected.clone())
                        .unwrap()
                )
                .unwrap(),
                expected
            );
        }
    }

    #[test]
    fn native_compaction_replays_the_entire_window_without_altering_opaque_or_retained_items() {
        let items = serde_json::json!([
            {"type": "message", "id": "msg-old", "role": "user", "content": [{"type": "input_text", "text": "keep my requirements"}], "future": [1, 2]},
            {"type": "compaction", "id": "cmp-1", "encrypted_content": "opaque-data", "future": {"a": true}},
            {"type": "message", "id": "msg-recent", "role": "assistant", "phase": "commentary", "status": "completed", "content": [{"type": "output_text", "text": "retained", "annotations": []}]}
        ]);
        let message = llm::Message {
            role: llm::Role::Assistant,
            content: vec![ContentBlock::OpenAICompaction(
                serde_json::from_value(items.clone()).unwrap(),
            )],
        };
        let request: ClientRequest = llm::ClientRequest::new(vec![message.clone()])
            .with_output_limit(2048)
            .try_into()
            .unwrap();
        assert_eq!(serde_json::to_value(request.input).unwrap(), items);
        assert_eq!(request.max_output_tokens, Some(2048));
        assert!(
            crate::claude::ClientRequest::try_from(llm::ClientRequest::new(vec![message])).is_err()
        );
    }

    #[test]
    fn incomplete_response_preserves_the_structured_reason() {
        for (reason, kind) in [
            ("max_output_tokens", FailureKind::Truncation),
            ("context_length_exceeded", FailureKind::ContextOverflow),
            ("content_filter", FailureKind::InvalidInput),
        ] {
            let event: openai::StreamEvent = serde_json::from_value(serde_json::json!({
                "type": "response.incomplete", "sequence_number": 1,
                "response": {"incomplete_details": {"reason": reason}}
            }))
            .unwrap();
            let Some(llm::StreamEvent::Error { error }) = Option::<llm::StreamEvent>::from(event)
            else {
                panic!("incomplete response must fail");
            };
            let failure = Failure::api(&error.error_type, &error.message);
            assert_eq!(failure.kind, kind);
            assert!(!failure.retryable());
        }
    }
}
