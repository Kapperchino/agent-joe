use crate::actor_state::{ActorState, StreamNextStep};
use crate::app::Command;
use crate::cache_actor::CacheActor;
use crate::cur_context::CurContext;
use crate::file_actor::FileActor;
use crate::llm::{ClientRequest, ContentBlock, LLmClient, StreamEvent};
use crate::tool_defs::ReadFile;
use crate::worker::Worker;
use crate::{app, cache_actor, file_actor, llm, tool_impls};
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
use tracing::error;

#[derive(Debug, Clone)]
pub enum ActorToTui {
    StateChanged(State),
    Data(String),
    ToolUse(Vec<String>),
    CommandResult(Command, String),
    TokensUpdated(TokenCount),
}

#[derive(Debug, Clone, Default)]
pub struct TokenCount {
    pub input_tokens: u32,
    pub output_tokens: u32,
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
    Err(anyhow::Error),
    Finished(),
}

#[derive(Debug)]
pub enum Message {
    StartWork(Option<String>),
    Command(app::Command),
    UseTool(Vec<(usize, Vec<StreamAccu>)>),
    ProcessStreamItem(StreamItem),
    KYS,
}

pub struct Dependency {
    pub claude: LLmClient,
    pub tools: Vec<tool_impls::Tool>,
    pub tui_tx: mpsc::UnboundedSender<ActorToTui>,
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
    Tool(tool_impls::ToolResult),
}

#[cfg_attr(feature = "async-trait", ractor::async_trait)]
impl Actor for Worker {
    type Msg = Message;
    type State = ActorState;
    type Arguments = Dependency;

    async fn pre_start(
        &self,
        myself: ActorRef<Self::Msg>,
        dependency: Dependency,
    ) -> Result<Self::State, ActorProcessingErr> {
        let cur_context = CurContext::new().await?;

        let (cache_actor_ref, _) = Actor::spawn_linked(
            None,
            CacheActor {},
            cache_actor::Dependency {
                symbol_cache: cur_context.symbol_cache.clone(),
                proj: cur_context.rust_proj.clone(),
            },
            myself.get_cell(),
        )
        .await?;

        let (file_actor_ref, _) = Actor::spawn_linked(
            None,
            FileActor {},
            file_actor::Dependency {
                main_dir: cur_context.cur_dir.clone(),
                vfs: cur_context.rust_proj.vfs.clone(),
                a_host: cur_context.rust_proj.analysis_host.clone(),
                cache_actor: cache_actor_ref,
            },
            myself.get_cell(),
        )
        .await?;

        let state = ActorState::new(dependency, cur_context, file_actor_ref)
            .await
            .actor_err()?;

        Ok(state)
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
                    state.history.push(llm::Message::new(p));
                });
                let req = ClientRequest::new(state.history.clone())
                    .with_thinking()
                    .with_tools(state.tools.clone());

                let stream = state.llm.chat_stream(req).await?;

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
                let tool_names: Vec<String> = vec
                    .iter()
                    .flat_map(|(_, accus)| accus.first())
                    .flat_map(|accu| match accu {
                        StreamAccu::Tool { name, .. } => Some(name.clone()),
                        _ => None,
                    })
                    .collect();

                if !tool_names.is_empty() {
                    state.tui_tx.send(ActorToTui::ToolUse(tool_names))?;
                }

                state.change_state(State::ToolStart);
                let res = state.process_tools(vec).await;
                state.save_history(res)?;
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
                    match state.process_stream_event(event) {
                        StreamNextStep::ToolUse => {
                            let mut vec: Vec<(usize, Vec<StreamAccu>)> =
                                state.acc_map.drain().into_iter().collect();

                            vec.sort_by(|(i1, _), (i2, _)| i1.cmp(i2));
                            if let Some(StreamAccu::Tool { .. }) =
                                vec.last().and_then(|(_, v)| v.first().cloned())
                            {
                                myself.send_message(Message::UseTool(vec))?;
                            } else {
                                let res = state.process_tools(vec).await;
                                state.save_history(res)?;
                            }
                        }
                        StreamNextStep::NewStream => {
                            // clear intermediate states
                            state.acc_map.clear();
                            state.delta_buf.clear();
                            myself.send_message(Message::StartWork(None))?;
                        }
                        StreamNextStep::Nothing => {}
                    }
                }
                StreamItem::Err(err) => {
                    error!("\nError: {:?}", err);
                }
                StreamItem::Finished() => {}
            },
            Message::KYS => myself.kill(),
            Message::Command(command) => match command {
                Command::PrintContext => {
                    let ctx = state.cur_context.get_ctx().await;
                    let _ = state.tui_tx.send(ActorToTui::CommandResult(command, ctx));
                }
            },
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
            SupervisionEvent::ActorTerminated(who, boxed_state, reason) => {
                if state
                    .stream_actor
                    .as_ref()
                    .map(|t| t.get_id() == who.get_id())
                    .unwrap_or(false)
                {
                    state.stream_actor = None;
                } else {
                    error!("{:?}", reason);
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
