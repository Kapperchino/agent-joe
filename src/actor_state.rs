use crate::actor::{ActorToTui, Dependency, State, StreamAccu, StreamRes, TokenCount};
use crate::cur_context::CurContext;
use crate::llm::{ContentBlock, LLmClient, Message, Role, StreamEvent};
use crate::tool_defs::{
    CargoCheckInput, InsertAfterLineInput, LenientDeserialize, ReadFileInput, StringReplaceInput,
    Tool, ToolJson, ToolResult,
};
use crate::{claude, file_actor, tool_impls};
use futures::{future, StreamExt};
use ra_ap_hir::sym::false_;
use ractor::{ActorCell, ActorRef};
use std::collections::HashMap;
use tokio::sync::mpsc;
use tracing::error;

pub struct ActorState {
    pub cur_context: CurContext,
    pub stream_actor: Option<ActorCell>,
    pub cur_state: State,
    pub history: Vec<Message>,
    pub claude: LLmClient,
    pub tools: Vec<Tool>,
    pub tools_json: Vec<ToolJson>,
    pub acc_map: HashMap<usize, Vec<StreamAccu>>,
    pub delta_buf: HashMap<usize, Vec<Delta>>,
    pub tui_tx: mpsc::UnboundedSender<ActorToTui>,
    pub file_actor: ActorRef<file_actor::Message>,
    pub token_count: TokenCount,
}
impl ActorState {
    pub async fn new(
        dependency: Dependency,
        cur_context: CurContext,
        file_actor: ActorRef<file_actor::Message>,
    ) -> anyhow::Result<Self> {
        let cur_context_str = cur_context.get_ctx().await;

        let computed_tools: Vec<ToolJson> = dependency.tools.iter().map(|t| t.to_json()).collect();

        Ok(Self {
            cur_context,
            cur_state: State::Ready,
            history: vec![Message::new(
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
            file_actor,
            token_count: TokenCount::default(),
        })
    }

    pub fn save_history(&mut self, vec: Vec<anyhow::Result<StreamRes>>) -> anyhow::Result<()> {
        vec.into_iter().try_for_each(|res| match res {
            Ok(stream_res) => match stream_res {
                StreamRes::String(str) => {
                    self.history.push(Message::new_assistant(str));
                    Ok(())
                }
                StreamRes::Thinking {
                    thinking,
                    signature,
                } => {
                    self.history.push(Message {
                        role: Role::Assistant,
                        content: vec![ContentBlock::ThinkingBlock {
                            thinking,
                            signature,
                        }],
                    });
                    Ok(())
                }
                StreamRes::Tool(tool_res) => {
                    let input = tool_res.tool().to_req()?;
                    self.history.push(Message {
                        role: Role::Assistant,
                        content: vec![ContentBlock::ToolBlock {
                            id: tool_res.tool().id(),
                            name: tool_res.tool().name(),
                            input,
                        }],
                    });
                    self.history.push(Message {
                        role: Role::User,
                        content: vec![tool_res.to_res_json()],
                    });
                    Ok(())
                }
            },
            Err(err) => {
                error!("{:?}", err);
                Err(err)
            }
        })?;
        Ok(())
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
                StreamNextStep::Nothing
            }
            StreamEvent::MessageStop {} => StreamNextStep::Nothing,
            StreamEvent::Error { error } => {
                error!("{:?}", error);
                StreamNextStep::Nothing
            }
            StreamEvent::MessageDelta { delta, usage } => match delta.stop_reason {
                Some(reason) => StreamNextStep::new(&reason),
                None => StreamNextStep::Nothing,
            },
            _ => StreamNextStep::Nothing,
        }
    }

    async fn tool_use(
        &self,
        a_vec: &Vec<StreamAccu>,
        name: String,
        id: String,
    ) -> anyhow::Result<ToolResult> {
        let json = match a_vec.get(1).ok_or(anyhow::Error::msg("doesn't work"))? {
            StreamAccu::Json(json) => Ok(json),
            _ => Err(anyhow::Error::msg("doesn't work")),
        }?;
        let id_c = id.clone();
        let tool = Tool::from_str(name.as_str())?;
        let res: anyhow::Result<ToolResult> = match Tool::from_str(name.as_str())? {
            Tool::ReadFile(_) => {
                let input = ReadFileInput::deserialize_lenient(json)?;
                let rf = tool_impls::ReadFile {
                    id: id.clone(),
                    input,
                };
                Ok(Tool::ReadFile(rf).use_tool(id, &self.cur_context).await?)
            }
            Tool::InsertAfterLine(_) => {
                let input = InsertAfterLineInput::deserialize_lenient(json)?;
                let rf = tool_impls::InsertAfterLine {
                    id: id.clone(),
                    input,
                };
                Ok(Tool::InsertAfterLine(rf)
                    .use_tool(id, &self.cur_context)
                    .await?)
            }
            Tool::StringReplace(_) => {
                let input = StringReplaceInput::deserialize_lenient(json)?;
                let rf = tool_impls::StringReplace {
                    id: id.clone(),
                    input,
                };
                Ok(Tool::StringReplace(rf)
                    .use_tool(id, &self.cur_context)
                    .await?)
            }
            Tool::CargoCheck(_) => {
                let input = if json.is_empty() {
                    CargoCheckInput {
                        include_warnings: None,
                    }
                } else {
                    CargoCheckInput::deserialize_lenient(json)?
                };
                let rf = tool_impls::CargoCheck {
                    id: id.clone(),
                    input,
                };
                Ok(Tool::CargoCheck(rf).use_tool(id, &self.cur_context).await?)
            }
        };
        match res {
            Ok(res) => Ok(res),
            Err(err) => Ok(ToolResult::Error {
                message: err.to_string(),
                tool,
                id: id_c,
            }),
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

pub enum StreamNextStep {
    // do nothing, normal path
    Nothing,
    ToolUse,
    // token ran out, need to restart the connection
    NewStream,
}

impl StreamNextStep {
    pub fn new(reason: &str) -> Self {
        match reason {
            "end_turn" => StreamNextStep::Nothing,
            "max_tokens" => StreamNextStep::NewStream,
            "stop_sequence" => StreamNextStep::Nothing,
            "tool_use" => StreamNextStep::ToolUse,
            "refusal" => StreamNextStep::ToolUse,
            _ => StreamNextStep::Nothing,
        }
    }
}
