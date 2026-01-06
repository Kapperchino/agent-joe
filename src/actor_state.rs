use crate::actor::{ActorToTui, Dependency, State, StreamAccu, StreamRes};
use crate::claude::{ClaudeClient, ContentBlock, ContentBlockInfo, Delta, Role, StreamEvent};
use crate::cur_context::CurContext;
use crate::tool_defs::{ReadFileInput, Tool, ToolResult, ToolResultTrait, ToolTrait};
use crate::{claude, tool_impls};
use futures::future;
use ractor::ActorCell;
use std::collections::HashMap;
use tokio::sync::mpsc;

pub struct ActorState {
    pub cur_context: CurContext,
    pub stream_actor: Option<ActorCell>,
    pub cur_state: State,
    pub history: Vec<claude::Message>,
    pub claude: ClaudeClient,
    pub tools: Vec<Tool>,
    pub tools_json: Vec<claude::Tool>,
    pub acc_map: HashMap<usize, Vec<StreamAccu>>,
    pub delta_buf: HashMap<usize, Vec<Delta>>,
    pub tui_tx: mpsc::UnboundedSender<ActorToTui>,
}

impl ActorState {
    pub async fn new(dependency: Dependency) -> anyhow::Result<Self> {
        let mut cur_context = CurContext::new().await?;
        let cur_context_str = cur_context.get_ctx().await;

        let computed_tools: Vec<claude::Tool> =
            dependency.tools.iter().map(|t| t.to_json()).collect();

        Ok(Self {
            cur_context,
            cur_state: State::Ready,
            history: vec![claude::Message::new(
                "This is the initial context in the environment: \n".to_owned()
                    + cur_context_str.as_str(),
            )],
            claude: dependency.claude,
            tools: dependency.tools,
            tools_json: computed_tools,
            acc_map: Default::default(),
            delta_buf: Default::default(),
            stream_actor: None,
            tui_tx: dependency.tui_tx,
        })
    }

    pub fn save_history(&mut self, vec: Vec<anyhow::Result<StreamRes>>) {
        vec.into_iter().for_each(|res| match res {
            Ok(stream_res) => match stream_res {
                StreamRes::String(str) => self.history.push(claude::Message::new_assistant(str)),
                StreamRes::Thinking {
                    thinking,
                    signature,
                } => {
                    self.history.push(claude::Message {
                        role: Role::Assistant,
                        content: vec![ContentBlock::ThinkingBlock {
                            thinking,
                            signature,
                        }],
                    });
                }
                StreamRes::Tool(tool_res) => {
                    self.history.push(claude::Message {
                        role: Role::Assistant,
                        content: vec![ContentBlock::ToolBlock {
                            id: tool_res.tool().id(),
                            name: tool_res.tool().name(),
                            input: tool_res.tool().to_req(),
                        }],
                    });
                    self.history.push(claude::Message {
                        role: Role::User,
                        content: vec![tool_res.to_res_json()],
                    });
                }
            },
            Err(err) => {
                println!("{:?}", err)
            }
        });
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

    pub fn handle_stream_state(&mut self, item: StreamEvent) {
        match item {
            StreamEvent::MessageStart { .. } => self.change_state(State::StreamStart),
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
                Delta::ThinkingDelta { thinking } => self.send_delta(thinking),
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

    async fn tool_use(
        &self,
        a_vec: &Vec<StreamAccu>,
        name: String,
        id: String,
    ) -> anyhow::Result<ToolResult> {
        match Tool::from_str(name.as_str())? {
            Tool::ReadFile(_) => match a_vec.get(1).ok_or(anyhow::Error::msg("doesn't work"))? {
                StreamAccu::Json(json) => {
                    let input: ReadFileInput = serde_json::from_str(json)?;
                    let rf = tool_impls::ReadFile {
                        id: id.clone(),
                        input,
                    };
                    Ok(Tool::ReadFile(rf).use_tool(id, &self.cur_context).await?)
                }
                _ => Err(anyhow::Error::msg("doesn't work")),
            },
        }
    }

    pub async fn process_tools(
        &self,
        vec: Vec<(usize, Vec<StreamAccu>)>,
    ) -> Vec<anyhow::Result<StreamRes>> {
        let futures: Vec<_> = vec
            .into_iter()
            .map(
                async |(_, a_vec): (usize, Vec<StreamAccu>)| match a_vec.first() {
                    Some(accu) => match accu {
                        StreamAccu::String(str) => Ok(StreamRes::String(str.clone())),
                        StreamAccu::Tool { id, name } => {
                            let tool_res = self
                                .tool_use(&a_vec, name.to_string(), id.to_string())
                                .await?;
                            Ok(StreamRes::Tool(tool_res))
                        }
                        StreamAccu::Thinking {
                            thinking,
                            signature,
                        } => Ok(StreamRes::Thinking {
                            thinking: thinking.clone(),
                            signature: signature.clone(),
                        }),
                        _ => Err(anyhow::Error::msg("No valid tool")),
                    },
                    None => Err(anyhow::Error::msg("No valid tool")),
                },
            )
            .collect();

        future::join_all(futures).await
    }
}
