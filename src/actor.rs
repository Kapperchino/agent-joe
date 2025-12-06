use crate::claude::{
    ClaudeClient, ClientRequest, ContentBlockInfo, Delta, StreamEvent, StreamMessage, Tool,
    ToolProperty, ToolSchemaDTO,
};
use anyhow::{Error, anyhow};
use log::{info, log};
use ractor::ActorRef;
use ractor::{Actor, MessagingErr};
use ractor::{ActorErr, ActorProcessingErr};
use std::collections::HashMap;
use thiserror::Error;
use tokio_stream::StreamExt;

const INITIAL_PROMPT: &str = "You are an agent making code changes to a rust codebase";

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
    UseTool(String),
    ContinueWork(),
}

pub struct ActorState {
    cur_state: State,
    history: Vec<String>,
    claude: ClaudeClient,
}

pub struct Dependency {
    pub(crate) claude: ClaudeClient,
}

pub enum State {
    Ready,
    Working,
    Stopped,
}
impl Message {}

#[derive(Debug)]
enum StreamAccu {
    String(String),
    Json(String),
    Tool(String),
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
        // startup the event processing
        Ok(ActorState {
            cur_state: State::Ready,
            history: vec![INITIAL_PROMPT.to_string()],
            claude: dependency.claude,
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
                let req = ClientRequest::new(vec![crate::claude::Message::new(prompt)])
                    .with_thinking()
                    .with_tools(vec![Tool {
                        name: "read_file".to_string(),
                        description: "".to_string(),
                        input_schema: ToolSchemaDTO {
                            name: "read_file".to_string(),
                            tool_type: "object".to_string(),
                            properties: HashMap::from([(
                                "file_path".to_string(),
                                ToolProperty {
                                    name: "file_path".to_string(),
                                    prop_type: "string".to_string(),
                                    description: "file path of the file you want to read"
                                        .to_string(),
                                },
                            )]),
                            required: vec!["file_path".into()],
                        },
                    }]);

                let mut stream = std::pin::pin!(state.claude.chat_stream(req));
                let mut map: HashMap<usize, Vec<Delta>> = HashMap::new();
                let mut acc_map: HashMap<usize, Vec<StreamAccu>> = HashMap::new();
                while let Some(event_result) = stream.next().await {
                    match event_result {
                        Ok(event) => match event {
                            StreamEvent::ContentBlockDelta { index, delta } => {
                                match map.get_mut(&index) {
                                    None => {
                                        map.insert(index, vec![delta]);
                                    }
                                    Some(vec) => vec.push(delta),
                                }
                            }
                            StreamEvent::ContentBlockStart {
                                index,
                                content_block,
                            } => match content_block {
                                ContentBlockInfo::ToolUse { id, input, name } => {
                                    match acc_map.get_mut(&index) {
                                        None => {
                                            acc_map.insert(index, vec![StreamAccu::Tool(name)]);
                                        }
                                        Some(vec) => vec.push(StreamAccu::Tool(name)),
                                    }
                                }
                                _ => {}
                            },
                            StreamEvent::ContentBlockStop { index } => {
                                map.remove(&index)
                                    .and_then(|buf| {
                                        buf.into_iter()
                                            .filter_map(|delta| match delta {
                                                Delta::TextDelta { text } => {
                                                    Some(StreamAccu::String(text))
                                                }
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
                                                    (
                                                        StreamAccu::Json(buffer),
                                                        StreamAccu::Json(delta),
                                                    ) => buffer.push_str(&delta),
                                                    _ => {
                                                        unreachable!(
                                                            "mixed Text and InputJson deltas"
                                                        )
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
                            StreamEvent::MessageStop {} => {
                                println!("{:?}",acc_map);
                                myself.stop(None)
                            },
                            StreamEvent::Error { .. } => {}
                            _ => {}
                        },
                        Err(e) => {
                            eprintln!("\nError: {:?}", e);
                            break;
                        }
                    }
                }
            }
            Message::UseTool(_) => {}
            Message::ContinueWork() => {}
        }

        Ok(())
    }
}

// pub async fn run() {
//     let (_, actor_handle) = Actor::spawn(None, Worker {}, ())
//         .await
//         .expect("Failed to start actor");
//     actor_handle.await.expect("Actor failed to exit cleanly");
// }
