use crate::claude::{ClaudeClient, ClientRequest, Tool, ToolProperty, ToolSchemaDTO};
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
pub struct Worker {
}

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
                        name: "pwd".to_string(),
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
                while let Some(event_result) = stream.next().await {
                    match event_result {
                        Ok(event) => {
                            println!("{:?}", event)
                        }
                        Err(e) => {
                            eprintln!("\nError: {:?}", e);
                            break;
                        }
                    }
                }
                myself.stop(None)
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
