use crate::actor::{Message, StreamAccu};
use anyhow::anyhow;
use clients::llm::{ContentBlockInfo, Delta, StreamEvent};
use clients::tool_defs::{ToolId, ToolUse};
use common_models::tui_models::{ActorToTui, State, TokenCount};
use std::collections::HashMap;
use std::path::PathBuf;
use tokio::fs::OpenOptions;
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;
use tracing::error;

struct StreamProcessor {
    pub acc_map: HashMap<usize, Vec<StreamAccu>>,
    pub delta_buf: HashMap<usize, Vec<Delta>>,
    pub stream_log_path: Option<PathBuf>,
    pub token_count: TokenCount,
    pub tui_tx: mpsc::UnboundedSender<ActorToTui>,
    pub cur_state: State,
}

pub struct ToolCall {
    id: ToolId,
    name: String,
    json: String,
}

pub enum StreamNextStep {
    // do nothing, normal path
    Nothing,
    ToolUse(ToolCall),
    // token ran out, need to restart the connection
    NewStream,
}

impl StreamNextStep {
    pub fn new(reason: &str, tool_call: Option<ToolCall>) -> anyhow::Result<Self> {
        match reason {
            "end_turn" => Ok(StreamNextStep::Nothing),
            "max_tokens" => Ok(StreamNextStep::NewStream),
            "stop_sequence" => Ok(StreamNextStep::Nothing),
            "tool_use" => Ok(StreamNextStep::ToolUse(
                tool_call.ok_or(anyhow!("Tool needs to be here"))?,
            )),
            "continue" => Ok(StreamNextStep::NewStream),
            "refusal" => Ok(StreamNextStep::ToolUse(
                tool_call.ok_or(anyhow!("Tool needs to be here"))?,
            )),
            _ => Ok(StreamNextStep::Nothing),
        }
    }
}
impl StreamProcessor {
    pub fn process_stream_event(&mut self, item: StreamEvent) -> StreamNextStep {
        match item {
            StreamEvent::ContentBlockDelta { index, delta } => {
                match self.delta_buf.get_mut(&index) {
                    None => {
                        self.delta_buf.insert(index, vec![delta]);
                    }
                    Some(vec) => vec.push(delta),
                }
                StreamNextStep::Nothing
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
                    StreamNextStep::Nothing
                }
                _ => StreamNextStep::Nothing,
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
                StreamNextStep::Nothing
            }
            StreamEvent::MessageStop {} => StreamNextStep::Nothing,
            StreamEvent::Error { error } => {
                error!("{:?}", error);
                StreamNextStep::Nothing
            }
            StreamEvent::MessageDelta { delta, usage } => match delta.stop_reason {
                Some(reason) => {
                    StreamNextStep::new(&reason,None)?
                }
                None => StreamNextStep::Nothing,
            },
            _ => StreamNextStep::Nothing,
        }
    }

    pub fn handle_stream_state(&mut self, item: StreamEvent) {
        match item {
            StreamEvent::MessageStart { message } => {
                self.change_state(State::StreamStart);
                self.token_count.input_tokens += message.usage.input_tokens;
                let _ = self
                    .tui_tx
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
                Delta::TextDelta { text } => self.send_delta(text),
                Delta::ThinkingDelta { thinking, .. } => self.send_delta(thinking),
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
                    .tui_tx
                    .send(ActorToTui::TokensUpdated(self.token_count.clone()));
            }
            StreamEvent::MessageStop => self.change_state(State::StreamStop),
            StreamEvent::Ping => {}
            StreamEvent::Error { .. } => {}
        }
    }

    pub fn change_state(&mut self, new_state: State) {
        self.cur_state = new_state.clone();
        let _ = self
            .tui_tx
            .send(ActorToTui::StateChanged(new_state.clone()));
    }
    pub fn send_delta(&mut self, str: String) {
        let _ = self.tui_tx.send(ActorToTui::Data(str));
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
