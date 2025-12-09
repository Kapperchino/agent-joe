use crate::claude::{
    ClaudeClient, ClaudeResult, ClientRequest, ContentBlock, ContentBlockInfo, Delta, Role,
    StreamEvent, StreamMessage, Tool, ToolBlock, ToolProperty, ToolSchemaDTO,
};
use crate::tools::{ReadFile, ToolResult, ToolTrait};
use crate::{claude, tools};
use anyhow::{Error, anyhow};
use futures::future;
use futures::future::try_join_all;
use log::{info, log};
use ra_ap_syntax::ast::make::name;
use ractor::ActorRef;
use ractor::{Actor, MessagingErr};
use ractor::{ActorErr, ActorProcessingErr};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::env;
use std::path::PathBuf;
use thiserror::Error;
use tokio_stream::{Stream, StreamExt};
use uuid::Uuid;

#[derive(Error, Debug)]
pub enum WorkerError {
    #[error("Claude API error: {0}")]
    Claude(#[from] crate::claude::ClaudeError),

    #[error("Still working")]
    WIP,

    #[error("Actor already stopped")]
    Ended,

    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

// Define a trait to convert errors
trait IntoActorErr<T> {
    fn actor_err(self) -> Result<T, ActorProcessingErr>;
}

impl<T, E: std::fmt::Display> IntoActorErr<T> for Result<T, E> {
    fn actor_err(self) -> Result<T, ActorProcessingErr> {
        self.map_err(|e| ActorProcessingErr::from(e.to_string()))
    }
}

// Base unit for the agent, should be given context and then simply do the work
pub struct Worker {}

#[derive(Debug, Clone)]
pub enum Message {
    StartWork(String),
    UseTool(Vec<(usize, Vec<StreamAccu>)>),
    ContinueWork(),
}

pub struct ActorState {
    cur_dir: PathBuf,
    cur_state: State,
    history: Vec<claude::Message>,
    claude: ClaudeClient,
    tools: Vec<tools::Tool>,
}

pub struct Dependency {
    pub(crate) claude: ClaudeClient,
    pub tools: Vec<tools::Tool>,
}

pub enum State {
    Ready,
    Working,
    Stopped,
}
impl Message {}

#[derive(Debug, Clone)]
pub enum StreamAccu {
    String(String),
    Json(String),
    Tool { id: String, name: String },
}

#[derive(Debug)]
enum StreamRes {
    String(String),
    Tool(tools::ToolResult),
}

#[cfg_attr(feature = "async-trait", ractor::async_trait)]
impl Actor for Worker {
    type Msg = Message;
    type State = ActorState;
    type Arguments = Dependency;

    async fn pre_start(
        &self,
        _: ActorRef<Self::Msg>,
        dependency: Dependency,
    ) -> Result<Self::State, ActorProcessingErr> {
        let current_dir = env::current_dir()?;

        // startup the event processing
        Ok(ActorState {
            cur_dir: current_dir,
            cur_state: State::Ready,
            history: vec![],
            claude: dependency.claude,
            tools: dependency.tools,
        })
    }

    async fn handle(
        &self,
        myself: ActorRef<Self::Msg>,
        message: Self::Msg,
        state: &mut Self::State,
    ) -> Result<(), ActorProcessingErr> {
        match state.cur_state {
            State::Working => {
                if let Message::StartWork(_) = message {
                    return Err(WorkerError::WIP).actor_err();
                }
            }
            State::Stopped => return Err(ActorProcessingErr::from(WorkerError::Ended.to_string())),
            _ => {}
        };

        match message {
            Message::StartWork(prompt) => {
                state.history.push(claude::Message::new(prompt.clone()));
                let tools: Vec<_> = state.tools.iter().map(|t| t.to_json()).collect();
                let req = ClientRequest::new(vec![crate::claude::Message::new(prompt.clone())])
                    .with_thinking()
                    .with_tools(tools);

                let acc_map = self.process_stream(state.claude.chat_stream(req)).await;

                let mut vec: Vec<(usize, Vec<StreamAccu>)> = acc_map.into_iter().collect();
                vec.sort_by(|(i1, _), (i2, _)| i1.cmp(i2));
                println!("{:?}", vec);

                myself.send_message(Message::UseTool(vec))?;
            }
            Message::UseTool(vec) => {
                let futures: Vec<_> = vec
                    .into_iter()
                    .map(
                        async |(_, a_vec): (usize, Vec<StreamAccu>)| match a_vec.first() {
                            Some(accu) => match accu {
                                StreamAccu::String(str) => Ok(StreamRes::String(str.clone())),
                                StreamAccu::Tool { id, name } => {
                                    let tool_res =
                                        Worker::tool_use(&a_vec, name.to_string(), id.to_string())
                                            .await?;
                                    Ok(StreamRes::Tool(tool_res))
                                }
                                _ => Err(anyhow::Error::msg("No valid tool")),
                            },
                            None => Err(anyhow::Error::msg("No valid tool")),
                        },
                    )
                    .collect();

                let res = future::join_all(futures).await;
                res.into_iter().for_each(|res| match res {
                    Ok(stream_res) => match stream_res {
                        StreamRes::String(str) => {
                            state.history.push(claude::Message::new_assistant(str))
                        }
                        StreamRes::Tool(tool_res) => {
                            let uuid = Uuid::new_v4();
                            let tool_id = format!("tool_use_{uuid}");
                            state.history.push(claude::Message {
                                role: Role::Assistant,
                                content: vec![ContentBlock::ToolBlock(ToolBlock {
                                    content_type: "tool_use".to_string(),
                                    id: tool_id,
                                    name: tool_res.tool().name(),
                                    input: tool_res.tool().to_req(),
                                })],
                            })
                        }
                    },
                    Err(err) => {
                        println!("{:?}", err)
                    }
                });
                println!("{:?}", state.history);
                myself.stop(None);
            }
            Message::ContinueWork() => {}
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
struct ToolInput {
    map: HashMap<String, String>,
}
impl Worker {
    pub async fn process_stream(
        &self,
        stream: impl Stream<Item = ClaudeResult<StreamEvent>> + Send + 'static,
    ) -> HashMap<usize, Vec<StreamAccu>> {
        let mut stream = std::pin::pin!(stream);
        let mut map: HashMap<usize, Vec<Delta>> = HashMap::new();
        let mut acc_map: HashMap<usize, Vec<StreamAccu>> = HashMap::new();
        while let Some(event_result) = stream.next().await {
            match event_result {
                Ok(event) => match event {
                    StreamEvent::ContentBlockDelta { index, delta } => match map.get_mut(&index) {
                        None => {
                            map.insert(index, vec![delta]);
                        }
                        Some(vec) => vec.push(delta),
                    },
                    StreamEvent::ContentBlockStart {
                        index,
                        content_block,
                    } => match content_block {
                        ContentBlockInfo::ToolUse { id, input, name } => {
                            match acc_map.get_mut(&index) {
                                None => {
                                    acc_map.insert(index, vec![StreamAccu::Tool { id, name }]);
                                }
                                Some(vec) => vec.push(StreamAccu::Tool { id, name }),
                            }
                        }
                        _ => {}
                    },
                    StreamEvent::ContentBlockStop { index } => {
                        map.remove(&index)
                            .and_then(|buf| {
                                buf.into_iter()
                                    .filter_map(|delta| match delta {
                                        Delta::TextDelta { text } => Some(StreamAccu::String(text)),
                                        Delta::InputJsonDelta { partial_json } => {
                                            Some(StreamAccu::Json(partial_json))
                                        }
                                        _ => None,
                                    })
                                    .reduce(|mut acc, delta| {
                                        match (&mut acc, delta) {
                                            (
                                                StreamAccu::String(buffer),
                                                StreamAccu::String(delta),
                                            ) => buffer.push_str(&delta),
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
                            .map(|buf| match acc_map.get_mut(&index) {
                                Some(vec) => vec.push(buf),
                                None => {
                                    acc_map.insert(index, vec![buf]);
                                }
                            });
                    }
                    StreamEvent::MessageStop {} => {}
                    StreamEvent::Error { .. } => {}
                    _ => {}
                },
                Err(e) => {
                    eprintln!("\nError: {:?}", e);
                    break;
                }
            }
        }
        acc_map
    }
    pub async fn tool_use(
        a_vec: &Vec<StreamAccu>,
        name: String,
        id: String,
    ) -> Result<ToolResult, anyhow::Error> {
        match tools::Tool::from_str(name.as_str())? {
            tools::Tool::ReadFile(_) => {
                match a_vec.get(1).ok_or(anyhow::Error::msg("doesn't work"))? {
                    StreamAccu::Json(json) => {
                        let rf: ReadFile = serde_json::from_str::<_>(json)?;
                        Ok(tools::Tool::ReadFile(rf).use_tool(id).await?)
                    }
                    _ => Err(anyhow::Error::msg("doesn't work")),
                }
            }
        }
    }
}
