use crate::actor_state::ActorState;
use crate::cache_actor::CacheActor;
use crate::file_actor::FileActor;
use crate::stream_processor::{PreprocessedStreamItem, ProcessedItem, StreamNextStep, ToolCall};
use crate::worker::Worker;
use crate::{cache_actor, file_actor};
use analysis::cur_context::CurContext;
use clients::llm;
use clients::llm::{ClientRequest, LLmClient, StreamEvent};
use clients::tool_defs::{
    CargoCheckInput, InsertAfterLineInput, ReadFileInput, StringReplaceInput, Tool, ToolResult,
};
use common_models::tui_models::ActorToTui;
use common_models::tui_models::Command;
use common_models::tui_models::State;
use ractor::Actor;
use ractor::ActorProcessingErr;
use ractor::ActorRef;
use ractor::SupervisionEvent;
use ractor_actors::streams::spawn_stream_pump;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use thiserror::Error;
use tokio::sync::mpsc;
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
    Command(Command),
    UseTool(Vec<PreprocessedStreamItem>),
    Noop(Vec<PreprocessedStreamItem>),
    ProcessStreamItem(StreamItem),
    KYS,
}

pub struct Dependency {
    pub claude: LLmClient,
    pub tools: Vec<Tool>,
    pub tui_tx: mpsc::UnboundedSender<ActorToTui>,
    pub log_streams: bool,
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
            Message::Noop(res) => {
                let res = state.stream_items_to_res(res).await;
                state.save_history(res)?;
            }
            Message::UseTool(vec) => {
                let tool_lines: Result<Vec<String>, anyhow::Error> = vec
                    .iter()
                    .filter_map(|x| {
                        if let ProcessedItem::Tool(t) = &x.processed {
                            Some(t.get_tool().map(|t| t.to_string()))
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
                            state.acc_map.clear();
                            state.delta_buf.clear();
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
