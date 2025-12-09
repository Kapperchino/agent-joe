use crate::claude::{
    ClaudeClient, ClaudeResult, ClientRequest, ContentBlock, ContentBlockInfo, Delta, Role,
    StreamEvent,
};
use crate::tools::{ReadFileInput, ToolResult, ToolTrait};
use crate::{claude, tools};
use anyhow::Error;
use futures::future;
use ractor::Actor;
use ractor::ActorProcessingErr;
use ractor::ActorRef;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::env;
use std::fmt::Display;
use std::path::PathBuf;
use thiserror::Error;
use tokio::fs;
use tokio::fs::DirEntry;
use tokio_stream::wrappers::ReadDirStream;
use tokio_stream::{Stream, StreamExt};

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
    StartWork(Option<String>),
    UseTool(Vec<(usize, Vec<StreamAccu>)>),
    ContinueWork(),
}

pub struct CurContext {
    cur_dir: PathBuf,
    cur_files: Vec<DirEntry>,
}

pub struct ActorState {
    cur_context: CurContext,
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
    Thinking { thinking: String, signature: String },
    Tool { id: String, name: String },
}

#[derive(Debug)]
enum StreamRes {
    String(String),
    Thinking { thinking: String, signature: String },
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
        let cur_context = CurContext::get_cur_context().await?;
        let cur_context_str = cur_context.to_string().await;
        // startup the event processing
        Ok(ActorState {
            cur_context,
            cur_state: State::Ready,
            history: vec![claude::Message::new(
                "This is the inital context in the enviornment: \n".to_owned()
                    + cur_context_str.as_str(),
            )],
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
                prompt.map(|p| {
                    state.history.push(claude::Message::new(p));
                });
                let tools: Vec<_> = state.tools.iter().map(|t| t.to_json()).collect();
                let req = ClientRequest::new(state.history.clone())
                    .with_thinking()
                    .with_tools(tools);

                let stream = state.claude.chat_stream(req).await?;

                let acc_map = self.process_stream(stream).await;

                let mut vec: Vec<(usize, Vec<StreamAccu>)> = acc_map.into_iter().collect();
                // vec.iter().for_each(|(_, acc)| {
                //     acc.iter().for_each(|s_acc| match s_acc {
                //         StreamAccu::String(joe) => {
                //             println!("{joe}")
                //         }
                //         StreamAccu::Json(_) => {}
                //         StreamAccu::Tool { .. } => {}
                //     })
                // });
                vec.sort_by(|(i1, _), (i2, _)| i1.cmp(i2));
                if let Some(StreamAccu::Tool { .. }) =
                    vec.last().and_then(|(_, v)| v.first().cloned())
                {
                    myself.send_message(Message::UseTool(vec))?;
                } else {    
                    myself.stop(None);
                }
            }
            Message::UseTool(vec) => {
                let res = Worker::process_tools(vec).await;
                res.into_iter().for_each(|res| match res {
                    Ok(stream_res) => match stream_res {
                        StreamRes::String(str) => {
                            state.history.push(claude::Message::new_assistant(str))
                        }
                        StreamRes::Thinking {
                            thinking,
                            signature,
                        } => {
                            state.history.push(claude::Message {
                                role: Role::Assistant,
                                content: vec![ContentBlock::ThinkingBlock {
                                    thinking,
                                    signature,
                                }],
                            });
                        }
                        StreamRes::Tool(tool_res) => {
                            state.history.push(claude::Message {
                                role: Role::Assistant,
                                content: vec![ContentBlock::ToolBlock {
                                    id: tool_res.tool().id(),
                                    name: tool_res.tool().name(),
                                    input: tool_res.tool().to_req(),
                                }],
                            });
                            state.history.push(claude::Message {
                                role: Role::User,
                                content: vec![tool_res.to_res_json()],
                            });
                        }
                    },
                    Err(err) => {
                        println!("{:?}", err)
                    }
                });
                // if tool result was the last value, then we can loop
                if let Some(ContentBlock::ToolResult { .. }) =
                    state.history.last().and_then(|msg| msg.content.last())
                {
                    myself.send_message(Message::StartWork(None))?;
                }
                println!("{:?}", state.history);
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

impl CurContext {
    async fn get_cur_context() -> Result<CurContext, anyhow::Error> {
        let current_dir = env::current_dir()?;
        let read_dir = fs::read_dir(current_dir.clone()).await?;
        let read_dir_stream = ReadDirStream::new(read_dir);
        let files = read_dir_stream
            .fold(vec![], |mut acc, item| {
                match item {
                    Ok(entry) => {
                        acc.push(entry);
                    }
                    Err(_) => {
                        println!("error with getting files")
                    }
                };
                acc
            })
            .await;
        Ok(CurContext {
            cur_dir: current_dir,
            cur_files: files,
        })
    }

    async fn to_string(&self) -> String {
        let dir = self.cur_dir.to_str().unwrap_or("");
        let files: Vec<_> = self
            .cur_files
            .iter()
            .map(async |file| {
                let path = file.path();
                let file_type = match file.file_type().await {
                    Ok(f_type) => {
                        if f_type.is_dir() {
                            Some("type: dir")
                        } else if f_type.is_file() {
                            Some("type: file")
                        } else if f_type.is_symlink() {
                            Some("type: symlink")
                        } else {
                            None
                        }
                    }
                    Err(_) => None,
                };
                if let Some(f_type) = file_type
                    && let Some(p_str) = path.to_str()
                {
                    Some(format!("path: {p_str}, {f_type}\n").to_string())
                } else {
                    None
                }
            })
            .collect();
        let res: String = future::join_all(files)
            .await
            .into_iter()
            .flatten()
            .fold(String::new(), |acc, s| format!("{acc}{s}").to_string());
        format!("Current Context: \ncurrent directory: {dir}\ncurrent files:\n{res}")
    }
}
impl Worker {
    async fn process_stream(
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
                                        Delta::ThinkingDelta { thinking } => {
                                            Some(StreamAccu::Thinking {
                                                thinking,
                                                signature: "".to_string(),
                                            })
                                        }
                                        Delta::SignatureDelta { signature } => {
                                            Some(StreamAccu::Thinking {
                                                thinking: "".to_string(),
                                                signature,
                                            })
                                        }
                                    })
                                    .reduce(|mut acc, delta| {
                                        match (&mut acc, delta) {
                                            (
                                                StreamAccu::String(buffer),
                                                StreamAccu::String(delta),
                                            ) => buffer.push_str(&delta),
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
                            .map(|buf| match acc_map.get_mut(&index) {
                                Some(vec) => vec.push(buf),
                                None => {
                                    acc_map.insert(index, vec![buf]);
                                }
                            });
                    }
                    StreamEvent::MessageStop {} => {}
                    StreamEvent::Error { error } => {
                        println!("{:?}", error)
                    }
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
    async fn tool_use(
        a_vec: &Vec<StreamAccu>,
        name: String,
        id: String,
    ) -> Result<ToolResult, anyhow::Error> {
        match tools::Tool::from_str(name.as_str())? {
            tools::Tool::ReadFile(_) => {
                match a_vec.get(1).ok_or(anyhow::Error::msg("doesn't work"))? {
                    StreamAccu::Json(json) => {
                        let input: ReadFileInput = serde_json::from_str::<_>(json)?;
                        let rf = tools::ReadFile {
                            id: id.clone(),
                            input,
                        };
                        Ok(tools::Tool::ReadFile(rf).use_tool(id).await?)
                    }
                    _ => Err(anyhow::Error::msg("doesn't work")),
                }
            }
        }
    }

    async fn process_tools(vec: Vec<(usize, Vec<StreamAccu>)>) -> Vec<Result<StreamRes, Error>> {
        let futures: Vec<_> = vec
            .into_iter()
            .map(
                async |(_, a_vec): (usize, Vec<StreamAccu>)| match a_vec.first() {
                    Some(accu) => match accu {
                        StreamAccu::String(str) => Ok(StreamRes::String(str.clone())),
                        StreamAccu::Tool { id, name } => {
                            let tool_res =
                                Worker::tool_use(&a_vec, name.to_string(), id.to_string()).await?;
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
