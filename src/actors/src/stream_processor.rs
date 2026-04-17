use crate::batch::Batch;
use crate::event_reporter::EventReporter;
use crate::tool_call::ToolCall;
use anyhow::{Error, anyhow};
use clients::llm::{ContentBlockInfo, Delta, StopReason, StreamEvent};
use clients::tool_defs::{
    CargoCheckInput, CargoTest, CargoTestInput, GrepInput, GrepTool, InsertAfterLine,
    InsertAfterLineInput, LenientDeserialize, ReadFile, ReadFileInput, StringReplace,
    StringReplaceInput, Tool, ToolId, ToolUse,
};
use common_models::tui_models::{ActorToTui, State, TokenCount};
use std::collections::HashMap;
use std::iter::Map;
use std::path::PathBuf;
use tokio::fs::OpenOptions;
use tokio::io::AsyncWriteExt;
use tracing::error;

pub struct StreamProcessor {
    pub batches: Vec<Batch>,
    pub stream_log_path: Option<PathBuf>,
    pub token_count: TokenCount,
    pub reporter: EventReporter,
    pub cur_state: State,
}

pub enum StreamNextStep {
    Accum,
    Done,
    ToolUse,
    Noop,
    // token ran out, need to restart the connection
    NewStream,
}

#[derive(Debug, Clone)]
pub enum StreamAccu {
    String(String),
    Json(String),
    Thinking {
        thinking: String,
        signature: String,
        reasoning_id: Option<String>,
    },
    Tool {
        id: ToolId,
        name: String,
    },
}

#[derive(Debug)]
pub struct PreprocessedStreamItem {
    pub index: usize,
    pub processed: ProcessedItem,
}
#[derive(Debug, Clone)]
pub enum ProcessedItem {
    String(String),
    Thinking {
        thinking: String,
        signature: String,
        reasoning_id: Option<String>,
    },
    Tool(ToolCall),
}

impl StreamNextStep {
    pub fn new(reason: &StopReason, batch: &Batch) -> anyhow::Result<Self> {
        match batch {
            Batch::Tool(_) => Ok(StreamNextStep::ToolUse),
            _ => match reason {
                StopReason::EndTurn => Ok(StreamNextStep::Done),
                StopReason::MaxTokens => Ok(StreamNextStep::NewStream),
                StopReason::StopSequence => Ok(StreamNextStep::Done),
                StopReason::ToolUse => Ok(StreamNextStep::ToolUse),
                StopReason::Refusal => {
                    error!("Stream refusal");
                    Ok(StreamNextStep::Done)
                }
                _ => Ok(StreamNextStep::Done),
            },
        }
    }
}
impl StreamProcessor {
    pub async fn process_stream_event(
        &mut self,
        item: StreamEvent,
    ) -> anyhow::Result<StreamNextStep> {
        self.log_stream_item(&item).await;
        self.handle_stream_state(&item);
        match item {
            StreamEvent::ContentBlockDelta { index, delta } => {
                self.batches
                    .last_mut()
                    .map(|batch| batch.accum(index, delta));
                Ok(StreamNextStep::Accum)
            }
            StreamEvent::ContentBlockStart {
                index,
                content_block,
            } => {
                self.batches.push(Batch::new(index, content_block));
                Ok(StreamNextStep::Accum)
            }
            StreamEvent::ContentBlockStop { index, id } => {
                self.batches
                    .last_mut()
                    .map(|batch| batch.apply_reduce(index, id));
                Ok(StreamNextStep::Accum)
            }
            StreamEvent::MessageStop {} => Ok(StreamNextStep::Noop),
            StreamEvent::Error { error } => {
                error!("{:?}", error);
                Ok(StreamNextStep::Noop)
            }
            StreamEvent::MessageDelta { delta, usage } => {
                if let Some(batch) = self.batches.pop()
                    && let Some(reason) = delta.stop_reason
                {
                    StreamNextStep::new(&reason, &batch)
                } else {
                    error!("No reason provided to stop");
                    Ok(StreamNextStep::Noop)
                }
            }
            StreamEvent::Accum => Ok(StreamNextStep::Accum),
            _ => Ok(StreamNextStep::Noop),
        }
    }

    pub fn handle_stream_state(&mut self, item: &StreamEvent) {
        match item {
            StreamEvent::MessageStart { message } => {
                self.change_state(State::StreamStart);
                self.token_count.input_tokens += message.usage.input_tokens;
                self.reporter
                    .send(ActorToTui::TokensUpdated(self.token_count.clone()));
            }
            StreamEvent::ContentBlockStart {
                index: _,
                content_block,
            } => match content_block {
                ContentBlockInfo::ToolUse { .. } => self.change_state(State::ToolStart),
                ContentBlockInfo::Thinking { .. } => self.change_state(State::ThinkingStart),
                ContentBlockInfo::Text { .. } => self.change_state(State::MessageStart),
            },
            StreamEvent::ContentBlockDelta { index, delta } => match delta {
                Delta::TextDelta { text } => self.reporter.send_delta(text.clone()),
                Delta::ThinkingDelta { thinking, .. } => self.reporter.send_delta(thinking.clone()),
                Delta::InputJsonDelta { .. } => {}
                Delta::SignatureDelta { .. } => {}
            },
            StreamEvent::ContentBlockStop { index, .. } => {
                self.batches
                    .last()
                    .and_then(|t| t.get_delta_type(index))
                    .inspect(|t| match t {
                        Delta::TextDelta { .. } => self.change_state(State::MessageStop),
                        Delta::ThinkingDelta { .. } => self.change_state(State::ThinkingStop),
                        Delta::InputJsonDelta { .. } => self.change_state(State::ToolStop),
                        Delta::SignatureDelta { .. } => {}
                    });
            }
            StreamEvent::MessageDelta { usage, .. } => {
                self.token_count.output_tokens += usage.output_tokens;
                self.token_count.input_tokens += usage.input_tokens;
                let _ = self
                    .reporter
                    .send(ActorToTui::TokensUpdated(self.token_count.clone()));
            }
            StreamEvent::MessageStop => self.change_state(State::StreamStop),
            StreamEvent::Ping => {}
            StreamEvent::Error { .. } => {}
            _ => {}
        }
    }
    
    pub fn clear(&mut self) {
        self.batches.clear();
        self.cur_state = State::Ready
    }

    pub fn change_state(&mut self, new_state: State) {
        self.cur_state = new_state.clone();
        self.reporter.state_changed(new_state.clone())
    }
    pub fn extract_and_pre_process(&mut self) -> anyhow::Result<Vec<PreprocessedStreamItem>> {
        match self.batches.pop() {
            Some(batch) => batch.extract_and_pre_process(),
            None => Err(anyhow!("Can't extract this, batch shouldn't be empty")),
        }
    }

    async fn log_stream_item(&self, item: &StreamEvent) {
        if let Some(ref path) = self.stream_log_path {
            if let Ok(mut file) = OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .await
            {
                if let Ok(json) = serde_json::to_string(item) {
                    let mut line = json;
                    line.push('\n');
                    let _ = file.write_all(line.as_bytes()).await;
                }
            }
        }
    }
}
