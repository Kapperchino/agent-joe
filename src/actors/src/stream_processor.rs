use crate::actor::Message;
use crate::event_reporter::EventReporter;
use anyhow::anyhow;
use clients::llm::{ContentBlockInfo, Delta, StopReason, StreamEvent};
use clients::tool_defs::{ToolId, ToolUse};
use common_models::tui_models::{ActorToTui, State, TokenCount};
use std::collections::HashMap;
use std::path::PathBuf;
use tokio::fs::OpenOptions;
use tokio::io::AsyncWriteExt;
use tracing::error;

pub struct StreamProcessor {
    pub acc_map: HashMap<usize, Vec<StreamAccu>>,
    pub delta_buf: HashMap<usize, Vec<Delta>>,
    pub stream_log_path: Option<PathBuf>,
    pub token_count: TokenCount,
    pub reporter: EventReporter,
    pub cur_state: State,
}
#[derive(Debug, Clone)]
pub struct ToolCall {
    pub id: ToolId,
    pub name: String,
    pub json: String,
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
    pub fn new(reason: &StopReason) -> anyhow::Result<Self> {
        match reason {
            StopReason::EndTurn => Ok(StreamNextStep::Done),
            StopReason::MaxTokens => Ok(StreamNextStep::NewStream),
            StopReason::StopSequence => Ok(StreamNextStep::Done),
            StopReason::ToolUse => Ok(StreamNextStep::ToolUse),
            StopReason::Refusal => {
                error!("Stream refusal");
                Ok(StreamNextStep::Done)
            }
            _ => Ok(StreamNextStep::Done),
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
                match self.delta_buf.get_mut(&index) {
                    None => {
                        self.delta_buf.insert(index, vec![delta]);
                    }
                    Some(vec) => vec.push(delta),
                }
                Ok(StreamNextStep::Accum)
            }
            StreamEvent::ContentBlockStart {
                index,
                content_block,
            } => match content_block {
                ContentBlockInfo::ToolUse { id, input, name } => {
                    match self.acc_map.get_mut(&index) {
                        None => {
                            self.acc_map
                                .insert(index, vec![StreamAccu::Tool { id, name }]);
                        }
                        Some(vec) => vec.push(StreamAccu::Tool { id, name }),
                    }
                    Ok(StreamNextStep::Accum)
                }
                _ => Ok(StreamNextStep::Accum),
            },
            StreamEvent::ContentBlockStop { index } => {
                self.accumulate(index).map(|buf| {
                    match self.acc_map.get_mut(&index) {
                        Some(vec) => vec.push(buf.clone()),
                        None => {
                            self.acc_map.insert(index, vec![buf.clone()]);
                        }
                    };
                    buf
                });
                Ok(StreamNextStep::Accum)
            }
            StreamEvent::MessageStop {} => Ok(StreamNextStep::Noop),
            StreamEvent::Error { error } => {
                error!("{:?}", error);
                Ok(StreamNextStep::Noop)
            }
            StreamEvent::MessageDelta { delta, usage } => match delta.stop_reason {
                Some(reason) => Ok(StreamNextStep::new(&reason)?),
                None => Ok(StreamNextStep::Noop),
            },
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
                ContentBlockInfo::ToolUse { .. } => {}
                ContentBlockInfo::Thinking { .. } => self.change_state(State::ThinkingStart),
                ContentBlockInfo::Text { .. } => self.change_state(State::MessageStart),
            },
            StreamEvent::ContentBlockDelta { index, delta } => match delta {
                Delta::TextDelta { text } => self.reporter.send_delta(text.clone()),
                Delta::ThinkingDelta { thinking, .. } => self.reporter.send_delta(thinking.clone()),
                Delta::InputJsonDelta { .. } => {}
                Delta::SignatureDelta { .. } => {}
            },
            StreamEvent::ContentBlockStop { index } => {
                self.delta_buf
                    .get(&index)
                    .cloned()
                    .and_then(|vec| vec.first().cloned())
                    .inspect(|t| match t {
                        Delta::TextDelta { .. } => self.change_state(State::MessageStop),
                        Delta::ThinkingDelta { .. } => self.change_state(State::ThinkingStop),
                        Delta::InputJsonDelta { .. } => {}
                        Delta::SignatureDelta { .. } => {}
                    });
            }
            StreamEvent::MessageDelta { usage, .. } => {
                self.token_count.output_tokens += usage.output_tokens;
                let _ = self
                    .reporter
                    .send(ActorToTui::TokensUpdated(self.token_count.clone()));
            }
            StreamEvent::MessageStop => self.change_state(State::StreamStop),
            StreamEvent::Ping => {}
            StreamEvent::Error { .. } => {}
        }
    }

    pub fn change_state(&mut self, new_state: State) {
        self.cur_state = new_state.clone();
        self.reporter.state_changed(new_state.clone())
    }
    pub fn extract_and_pre_process(&mut self) -> anyhow::Result<Vec<PreprocessedStreamItem>> {
        let mut vec: Vec<(usize, Vec<StreamAccu>)> = self.acc_map.drain().into_iter().collect();
        vec.sort_by(|(i1, _), (i2, _)| i1.cmp(i2));
        let res: Result<_, _> = vec
            .into_iter()
            .map(|(i, item)| {
                let prep = if let Some(StreamAccu::Tool { .. }) = item.first() {
                    let toolcall = Self::extract_tool((i, item))?;
                    Ok(ProcessedItem::Tool(toolcall))
                } else {
                    match item.first().cloned() {
                        Some(accu) => Ok(match accu {
                            StreamAccu::String(s) => ProcessedItem::String(s),
                            StreamAccu::Thinking {
                                thinking,
                                signature,
                                reasoning_id,
                            } => ProcessedItem::Thinking {
                                thinking,
                                signature,
                                reasoning_id,
                            },
                            _ => unreachable!("Should not be here"),
                        }),
                        None => Err(anyhow!("Empty stream process")),
                    }
                }?;
                Ok(PreprocessedStreamItem {
                    index: i,
                    processed: prep,
                })
            })
            .collect();
        res
    }

    fn extract_tool((_, vec): (usize, Vec<StreamAccu>)) -> anyhow::Result<ToolCall> {
        let tool_info = vec
            .get(0)
            .cloned()
            .map(|t| match t {
                StreamAccu::Tool { id, name } => Ok(StreamAccu::Tool { id, name }),
                _ => Err(anyhow::Error::msg("Tool doesn't exist")),
            })
            .transpose()?;
        let json = vec
            .get(1)
            .cloned()
            .map(|j| match j {
                StreamAccu::Json(json) => Ok(json),
                _ => Err(anyhow::Error::msg("Json doesn't exist")),
            })
            .transpose()?;

        if let Some(StreamAccu::Tool { id, name }) = tool_info
            && let Some(json) = json
        {
            Ok(ToolCall { id, name, json })
        } else {
            Err(anyhow::Error::msg("Type shit"))
        }
    }

    fn accumulate(&mut self, index: usize) -> Option<StreamAccu> {
        self.delta_buf.remove(&index).and_then(|buf| {
            buf.into_iter()
                .filter_map(|delta| match delta {
                    Delta::TextDelta { text } => Some(StreamAccu::String(text)),
                    Delta::InputJsonDelta { partial_json } => Some(StreamAccu::Json(partial_json)),
                    Delta::ThinkingDelta {
                        thinking,
                        reasoning_id,
                    } => Some(StreamAccu::Thinking {
                        thinking,
                        signature: "".to_string(),
                        reasoning_id,
                    }),
                    Delta::SignatureDelta { signature } => Some(StreamAccu::Thinking {
                        thinking: "".to_string(),
                        signature,
                        reasoning_id: None,
                    }),
                })
                .reduce(|mut acc, delta| {
                    match (&mut acc, delta) {
                        (StreamAccu::String(buffer), StreamAccu::String(delta)) => {
                            buffer.push_str(&delta)
                        }
                        (
                            StreamAccu::Thinking {
                                thinking: think_buf,
                                signature: sig,
                                reasoning_id: acc_id,
                            },
                            StreamAccu::Thinking {
                                thinking,
                                signature,
                                reasoning_id,
                            },
                        ) => {
                            think_buf.push_str(&thinking);
                            if !signature.is_empty() {
                                sig.push_str(&signature)
                            }
                            if reasoning_id.is_some() {
                                *acc_id = reasoning_id;
                            }
                        }
                        (StreamAccu::Json(buffer), StreamAccu::Json(delta)) => {
                            buffer.push_str(&delta)
                        }
                        _ => {
                            unreachable!("mixed Text and InputJson deltas")
                        }
                    }
                    acc
                })
        })
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
