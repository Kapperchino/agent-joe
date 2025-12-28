use crate::actor_state::ActorState;
use crate::claude::{ClaudeClient, ClaudeError, ClientRequest, ContentBlock, StreamEvent};
use crate::cur_context::CurContext;
use crate::worker::Worker;
use crate::{claude, tools};
use ractor::Actor;
use ractor::ActorProcessingErr;
use ractor::ActorRef;
use ractor::SupervisionEvent;
use ractor_actors::streams::spawn_stream_pump;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt::Display;
use thiserror::Error;
use tokio::sync::mpsc;
use tokio_stream::{Stream, StreamExt};

#[derive(Debug, Clone)]
pub enum ActorToTui {
    StateChanged(State),
    Data(String),
}
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

#[derive(Debug)]
pub enum StreamItem {
    Item(StreamEvent),
    Err(ClaudeError),
    Finished(),
}

#[derive(Debug)]
pub enum Message {
    StartWork(Option<String>),
    UseTool(Vec<(usize, Vec<StreamAccu>)>),
    ProcessStreamItem(StreamItem),
    KYS,
}

pub struct Dependency {
    pub claude: ClaudeClient,
    pub tools: Vec<tools::Tool>,
    pub tui_tx: mpsc::UnboundedSender<ActorToTui>,
    pub db_env: heed::Env
}

#[derive(Debug, Clone)]
pub enum State {
    Ready,
    StreamStart,
    StreamStop,
    ThinkingStart,
    ThinkingStop,
    MessageStart,
    MessageStop,
    ToolStart,
    ToolStop,
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
pub(crate) enum StreamRes {
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
        ActorState::new(dependency).await.actor_err()
    }

    async fn handle(
        &self,
        myself: ActorRef<Self::Msg>,
        message: Self::Msg,
        state: &mut Self::State,
    ) -> Result<(), ActorProcessingErr> {
        match message {
            Message::StartWork(prompt) => {
                prompt.map(|p| {
                    state.history.push(claude::Message::new(p));
                });
                let tools = state.tools_json.clone();
                let req = ClientRequest::new(state.history.clone())
                    .with_thinking()
                    .with_tools(tools);

                let stream = state.claude.chat_stream(req).await?;

                let actor = spawn_stream_pump(
                    stream,
                    myself,
                    |event| match event {
                        None => Message::ProcessStreamItem(StreamItem::Finished()),
                        Some(res) => match res {
                            Ok(item) => Message::ProcessStreamItem(StreamItem::Item(item)),
                            Err(err) => Message::ProcessStreamItem(StreamItem::Err(err)),
                        },
                    },
                    None,
                )
                .await?;

                state.stream_actor = Some(actor)
            }
            Message::UseTool(vec) => {
                let res = state.process_tools(vec).await;
                state.change_state(State::ToolStart);
                state.save_history(res);
                state.change_state(State::ToolStop);
                // if tool result was the last value, then we can loop
                if let Some(ContentBlock::ToolResult { .. }) =
                    state.history.last().and_then(|msg| msg.content.last())
                {
                    myself.send_message(Message::StartWork(None))?;
                }
            }
            Message::ProcessStreamItem(item) => match item {
                StreamItem::Item(event) => {
                    state.handle_stream_state(event.clone());
                    state.process_stream_event(event);
                }
                StreamItem::Err(err) => {
                    eprintln!("\nError: {:?}", err);
                }
                StreamItem::Finished() => {
                    let mut vec: Vec<(usize, Vec<StreamAccu>)> =
                        state.acc_map.drain().into_iter().collect();

                    vec.sort_by(|(i1, _), (i2, _)| i1.cmp(i2));
                    if let Some(StreamAccu::Tool { .. }) =
                        vec.last().and_then(|(_, v)| v.first().cloned())
                    {
                        myself.send_message(Message::UseTool(vec))?;
                    } else {
                        let res = state.process_tools(vec).await;
                        state.save_history(res);
                    }
                }
            },
            Message::KYS => myself.kill(),
        }
        Ok(())
    }

    async fn handle_supervisor_evt(
        &self,
        _myself: ActorRef<Self::Msg>,
        message: SupervisionEvent,
        state: &mut Self::State,
    ) -> Result<(), ActorProcessingErr> {
        match message {
            SupervisionEvent::ActorTerminated(who, _, _) => {
                if state
                    .stream_actor
                    .as_ref()
                    .map(|t| t.get_id() == who.get_id())
                    .unwrap_or(false)
                {
                    state.stream_actor = None;
                }
            }
            SupervisionEvent::ActorFailed(who, reason) => {
                eprintln!("Child actor {:?} failed: {:?}", who.get_id(), reason);
            }
            _ => {}
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
struct ToolInput {
    map: HashMap<String, String>,
}
