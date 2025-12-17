use crate::actor::{State, StreamAccu};
use crate::claude::{ClaudeClient, ContentBlockInfo, Delta, StreamEvent};
use crate::cur_context::CurContext;
use crate::{claude, tools};
use ractor::ActorCell;
use std::collections::HashMap;

pub struct ActorState {
    pub(crate) cur_context: CurContext,
    pub(crate) stream_actor: Option<ActorCell>,
    pub(crate) cur_state: State,
    pub(crate) history: Vec<claude::Message>,
    pub(crate) claude: ClaudeClient,
    pub(crate) tools: Vec<tools::Tool>,
    pub acc_map: HashMap<usize, Vec<StreamAccu>>,
    pub delta_buf: HashMap<usize, Vec<Delta>>,
}

impl ActorState {
    pub fn change_state(&mut self, new_state: State) {
        self.cur_state = new_state.clone();
        println!("{:?}", new_state)
    }
    pub fn handle_stream_state(&mut self, item: StreamEvent) {
        match item {
            StreamEvent::MessageStart { .. } => self.change_state(State::StreamStart),
            StreamEvent::ContentBlockStart {
                index,
                content_block,
            } => match content_block {
                ContentBlockInfo::ToolUse { .. } => {}
                ContentBlockInfo::Thinking { .. } => self.change_state(State::ThinkingStart),
                ContentBlockInfo::Text { .. } => self.change_state(State::MessageStart),
            },
            StreamEvent::ContentBlockDelta { .. } => {}
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
            StreamEvent::MessageDelta { .. } => {}
            StreamEvent::MessageStop => self.change_state(State::StreamStop),
            StreamEvent::Ping => {}
            StreamEvent::Error { .. } => {}
        }
    }
    pub fn process_stream_event(&mut self, item: StreamEvent) {
        match item {
            StreamEvent::ContentBlockDelta { index, delta } => {
                match self.delta_buf.get_mut(&index) {
                    None => {
                        self.delta_buf.insert(index, vec![delta]);
                    }
                    Some(vec) => vec.push(delta),
                }
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
                }
                _ => {}
            },
            StreamEvent::ContentBlockStop { index } => {
                self.delta_buf
                    .remove(&index)
                    .and_then(|buf| {
                        buf.into_iter()
                            .filter_map(|delta| match delta {
                                Delta::TextDelta { text } => Some(StreamAccu::String(text)),
                                Delta::InputJsonDelta { partial_json } => {
                                    Some(StreamAccu::Json(partial_json))
                                }
                                Delta::ThinkingDelta { thinking } => Some(StreamAccu::Thinking {
                                    thinking,
                                    signature: "".to_string(),
                                }),
                                Delta::SignatureDelta { signature } => Some(StreamAccu::Thinking {
                                    thinking: "".to_string(),
                                    signature,
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
                                        },
                                        StreamAccu::Thinking {
                                            thinking,
                                            signature,
                                        },
                                    ) => {
                                        think_buf.push_str(&thinking);
                                        if !signature.is_empty() {
                                            sig.push_str(&signature)
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
                    .map(|buf| match self.acc_map.get_mut(&index) {
                        Some(vec) => vec.push(buf),
                        None => {
                            self.acc_map.insert(index, vec![buf]);
                        }
                    });
            }
            StreamEvent::MessageStop {} => {}
            StreamEvent::Error { error } => {
                println!("{:?}", error)
            }
            _ => {}
        }
    }
}
