pub mod batch;
use anyhow::anyhow;
use batch::{Batch, ContentBlock, ContentKind};
use clients::llm::{ContentBlockInfo, Delta, StopReason, StreamEvent};
pub use clients::response::{ProcessedItem, StreamNextStep};
use common_models::tui_models::{ActorToTuiPacket, State, TokenCount};

pub struct StreamProcessor {
    pub batches: Vec<Batch>,
    pub token_count: TokenCount,
    pub cur_state: State,
}

impl Default for StreamProcessor {
    fn default() -> Self {
        Self {
            batches: Vec::new(),
            token_count: TokenCount::default(),
            cur_state: State::Ready,
        }
    }
}

pub enum StreamNotification {
    Packet(ActorToTuiPacket),
    Usage(TokenCount),
}

pub struct StreamUpdate {
    pub next: anyhow::Result<StreamNextStep>,
    pub notifications: Vec<StreamNotification>,
}

fn next_step(reason: &StopReason, batch: &Batch) -> anyhow::Result<StreamNextStep> {
    use clients::failure::{Failure, FailureKind};
    match reason {
        StopReason::Refusal => Ok(StreamNextStep::Refused),
        StopReason::MaxTokens => Err(Failure::new(
            FailureKind::Truncation,
            "Provider output reached its token limit; no pending tools were executed",
        )
        .into()),
        StopReason::ContextExceeded => Err(Failure::new(
            FailureKind::ContextOverflow,
            "Provider context limit exceeded",
        )
        .into()),
        StopReason::ToolUse => Ok(StreamNextStep::ToolUse),
        _ if batch.has_tool() => Ok(StreamNextStep::ToolUse),
        _ => Ok(StreamNextStep::Done),
    }
}
impl StreamProcessor {
    pub fn process_stream_event(&mut self, item: StreamEvent) -> StreamUpdate {
        let notifications = self.handle_stream_state(&item);
        StreamUpdate {
            next: self.accumulate(item),
            notifications,
        }
    }

    fn accumulate(&mut self, item: StreamEvent) -> anyhow::Result<StreamNextStep> {
        match item {
            StreamEvent::MessageStart { .. } => {
                self.batches.push(Batch::new());
                Ok(StreamNextStep::Started)
            }
            StreamEvent::ContentBlockDelta { index, delta } => {
                self.batches
                    .last_mut()
                    .ok_or_else(|| anyhow!("Delta without a response"))?
                    .accum(&index, delta)?;
                Ok(StreamNextStep::Accum)
            }
            StreamEvent::ContentBlockStart {
                index,
                content_block,
            } => {
                self.batches
                    .last_mut()
                    .ok_or_else(|| anyhow!("Content start without a response"))?
                    .put(index, ContentBlock::new(content_block));
                Ok(StreamNextStep::Accum)
            }
            StreamEvent::ContentBlockStop { index, id } => {
                self.batches
                    .last_mut()
                    .ok_or_else(|| anyhow!("Content stop without a response"))?
                    .apply_reduce(&index, id)?;
                Ok(StreamNextStep::Accum)
            }
            StreamEvent::ContentBlockComplete { index, content } => {
                self.batches
                    .last_mut()
                    .ok_or_else(|| anyhow!("Completed item without a response"))?
                    .complete_item(index, content);
                Ok(StreamNextStep::Accum)
            }
            StreamEvent::MessageStop {} => Ok(StreamNextStep::Noop),
            StreamEvent::Error { error } => {
                Err(clients::failure::Failure::api(&error.error_type, &error.message).into())
            }
            StreamEvent::MessageDelta { delta, .. } => {
                match (self.batches.last(), delta.stop_reason) {
                    (Some(batch), Some(reason)) => next_step(&reason, batch),
                    _ => Ok(StreamNextStep::Noop),
                }
            }
            StreamEvent::Accum => Ok(StreamNextStep::Accum),
            _ => Ok(StreamNextStep::Noop),
        }
    }

    fn handle_stream_state(&mut self, item: &StreamEvent) -> Vec<StreamNotification> {
        let mut notifications = Vec::new();
        match item {
            StreamEvent::MessageStart { message } => {
                notifications.push(self.change_state(State::StreamStart));
                self.record_usage(
                    &mut notifications,
                    TokenCount {
                        input_tokens: message.usage.input_tokens,
                        output_tokens: 0,
                    },
                );
            }
            StreamEvent::ContentBlockStart {
                index: _,
                content_block,
            } => match content_block {
                ContentBlockInfo::ToolUse { .. } => {
                    notifications.push(self.change_state(State::ToolStart))
                }
                ContentBlockInfo::Thinking { .. } => {
                    notifications.push(self.change_state(State::ThinkingStart))
                }
                ContentBlockInfo::Text { .. } => {
                    notifications.push(self.change_state(State::MessageStart))
                }
            },
            StreamEvent::ContentBlockDelta { delta, .. } => match delta {
                Delta::TextDelta { text } => notifications.push(StreamNotification::Packet(
                    ActorToTuiPacket::Data(text.clone()),
                )),
                Delta::ThinkingDelta { .. } => {}
                Delta::InputJsonDelta { .. } => {}
                Delta::SignatureDelta { .. } => {}
            },
            StreamEvent::ContentBlockStop { index, .. } => {
                self.batches
                    .last()
                    .and_then(|t| t.content_kind(index))
                    .inspect(|t| match t {
                        ContentKind::Text => {
                            notifications.push(self.change_state(State::MessageStop))
                        }
                        ContentKind::Thinking => {
                            notifications.push(self.change_state(State::ThinkingStop))
                        }
                        ContentKind::Tool => notifications.push(self.change_state(State::ToolStop)),
                    });
            }
            StreamEvent::MessageDelta { usage, .. } => {
                self.record_usage(
                    &mut notifications,
                    TokenCount {
                        input_tokens: usage.input_tokens,
                        output_tokens: usage.output_tokens,
                    },
                );
            }
            StreamEvent::ContentBlockComplete { content, .. } => {
                notifications.push(self.change_state(match content {
                    clients::llm::ContentBlock::OpenAIReasoning(_) => State::ThinkingStop,
                    clients::llm::ContentBlock::ToolBlock { .. } => State::ToolStop,
                    _ => State::MessageStop,
                }));
            }
            StreamEvent::MessageStop => notifications.push(self.change_state(State::StreamStop)),
            StreamEvent::Ping => {}
            StreamEvent::Error { .. } => {}
            _ => {}
        }
        notifications
    }

    pub fn clear(&mut self) {
        self.batches.clear();
        self.cur_state = State::Ready
    }

    fn record_usage(&mut self, notifications: &mut Vec<StreamNotification>, usage: TokenCount) {
        self.token_count.input_tokens = self
            .token_count
            .input_tokens
            .saturating_add(usage.input_tokens);
        self.token_count.output_tokens = self
            .token_count
            .output_tokens
            .saturating_add(usage.output_tokens);
        notifications.push(StreamNotification::Usage(usage));
        notifications.push(StreamNotification::Packet(ActorToTuiPacket::TokensUpdated(
            self.token_count.clone(),
        )));
    }

    pub fn change_state(&mut self, state: State) -> StreamNotification {
        self.cur_state = state.clone();
        StreamNotification::Packet(ActorToTuiPacket::StateChanged(state))
    }

    pub fn extract_and_pre_process(&mut self) -> anyhow::Result<Vec<ProcessedItem>> {
        match self.batches.last_mut() {
            Some(batch) => batch.extract_and_pre_process(),
            None => Err(anyhow!("Can't extract this, batch shouldn't be empty")),
        }
    }
}
