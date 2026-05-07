use crate::actor_state::ActorState;
use crate::stream_processor::{PreprocessedStreamItem, ProcessedItem, StreamNextStep};
use crate::worker::{Worker, WorkerAdapter};
use analysis::contexts::context::Context;
use clients::llm;
use clients::llm::{ClientRequest, LLmClient, StreamEvent};
use commands::command::Command;
use common_models::tui_models::ActorToTui;
use common_models::tui_models::State;
use futures::StreamExt;
use ractor::Actor;
use ractor::ActorProcessingErr;
use ractor::ActorRef;
use ractor::SupervisionEvent;
use ractor_actors::streams::spawn_stream_pump;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use thiserror::Error;
use tokio::sync::mpsc;
use tools::tool_defs::{ErasedToolRef, ToolResult};
use tracing::error;

#[derive(Error, Debug)]
pub enum WorkerError {
    #[error("Claude API error: {0}")]
    Claude(#[from] clients::claude::ClaudeError),

    #[error("Still working")]
    WIP,

    #[error("Actor already stopped")]
    Ended,

    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

// Define a trait to convert errors
pub trait IntoActorErr<T> {
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
    Command(Command),
    UseTool(Vec<PreprocessedStreamItem>),
    Noop(Vec<PreprocessedStreamItem>),
    ProcessStreamItem(StreamItem),
    Interrupt,
    Clear,
    KYS,
}

pub struct Dependency<C: Context> {
    pub client: LLmClient,
    pub tools: Vec<ErasedToolRef<C>>,
    pub tui_tx: mpsc::UnboundedSender<ActorToTui>,
    pub debug_mode: bool,
    pub context: C,
}

impl Message {}

#[derive(Debug)]
pub enum StreamRes {
    String(String),
    Thinking {
        thinking: String,
        signature: String,
        reasoning_id: Option<String>,
    },
    Tool(ToolResult),
}

#[cfg_attr(feature = "async-trait", ractor::async_trait)]
impl<W: Worker> Actor for WorkerAdapter<W> {
    type Msg = Message;
    type State = ActorState<W::C>;
    type Arguments = Dependency<W::C>;

    async fn pre_start(
        &self,
        myself: ActorRef<Self::Msg>,
        dependency: Self::Arguments,
    ) -> Result<Self::State, ActorProcessingErr> {
        self.worker.startup_hook(myself, dependency).await
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
                    .with_tools(state.tool_definitions())
                    .with_thinking();

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

                state.stream_actor.as_ref().map(|cell| cell.stop(None));

                state.stream_actor = Some(actor)
            }
            Message::Noop(res) => {
                let res = state.stream_items_to_res(res).await;
                state.save_history(res)?;
            }
            Message::UseTool(vec) => {
                let tool_lines: Result<Vec<String>, anyhow::Error> = vec
                    .iter()
                    .filter_map(|x| {
                        if let ProcessedItem::Tool(t) = &x.processed {
                            Some(state.tool_display(t))
                        } else {
                            None
                        }
                    })
                    .collect();
                let tool_lines = tool_lines?;

                if !tool_lines.is_empty() {
                    state.reporter.send(ActorToTui::ToolUse(tool_lines));
                }

                state.change_state(State::ToolStart);
                let res = state.process_tools(vec).await;
                state.save_history(res)?;
                state.change_state(State::ToolStop);
                myself.send_message(Message::StartWork(None))?;
            }
            Message::ProcessStreamItem(item) => match item {
                StreamItem::Item(event) => {
                    match state
                        .stream_processor
                        .process_stream_event(event.clone())
                        .await?
                    {
                        StreamNextStep::ToolUse => {
                            let pre_processed = state.stream_processor.extract_and_pre_process()?;
                            myself.send_message(Message::UseTool(pre_processed))?;
                        }
                        StreamNextStep::NewStream => {
                            // clear intermediate states
                            state.stream_processor.clear();
                            myself.send_message(Message::StartWork(None))?;
                        }
                        StreamNextStep::Done => {
                            let pre_processed = state.stream_processor.extract_and_pre_process()?;
                            myself.send_message(Message::Noop(pre_processed))?;
                        }
                        StreamNextStep::Accum => {}
                        StreamNextStep::Noop => {}
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
                    let _ = state.reporter.send(ActorToTui::CommandResult(command, ctx));
                }
                Command::Logout => match clients::config::Config::delete().await {
                    Ok(_) => {
                        let _ = state.reporter.send(ActorToTui::CommandResult(
                            command,
                            "Logged out. Removed config".to_string(),
                        ));
                    }
                    Err(err) => {
                        let _ = state.reporter.send(ActorToTui::CommandResult(
                            command,
                            format!("Deletion failed: {err}"),
                        ));
                    }
                },
                Command::Clear => {
                    myself.send_message(Message::Clear)?;
                    let _ = state.reporter.send(ActorToTui::CommandResult(
                        command,
                        "History cleared".to_string(),
                    ));
                }
                Command::ChangeModel(name, effort) => {
                    state.llm.change_model_and_effort(name, effort).await?;
                }
            },
            Message::Interrupt => {
                state.stream_processor.clear();
                state.stream_actor.as_ref().map(|cell| cell.stop(None));
                state.stream_actor = None;
                state.change_state(State::Ready);
            }
            Message::Clear => {
                state.history.clear();
                state.stream_processor.clear();
                state.stream_actor.as_ref().map(|cell| cell.stop(None));
                state.stream_actor = None;
                state.change_state(State::Ready);
            }
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
                error!("Child actor {:?} failed: {:?}", who.get_id(), reason);
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
