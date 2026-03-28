use crate::actor::{Dependency, StreamRes};
use crate::event_reporter::EventReporter;
use crate::file_actor;
use crate::stream_processor::{
    PreprocessedStreamItem, ProcessedItem, StreamAccu, StreamProcessor, ToolCall,
};
use analysis::cur_context::CurContext;
use anyhow::anyhow;
use clients::llm::{ContentBlock, Delta, LLmClient, Message, Role};
use clients::tool_defs::{
    CargoCheckInput, InsertAfterLineInput, LenientDeserialize, ReadFileInput, StringReplaceInput,
    Tool, ToolId, ToolResult,
};
use common_models::tui_models::State;
use futures::future;
use ractor::{ActorCell, ActorRef};
use std::collections::HashMap;
use std::path::PathBuf;
use tracing::{error, warn};

pub struct ActorState {
    pub cur_context: CurContext,
    pub stream_actor: Option<ActorCell>,
    pub history: Vec<Message>,
    pub llm: LLmClient,
    pub tools: Vec<Tool>,
    pub acc_map: HashMap<usize, Vec<StreamAccu>>,
    pub delta_buf: HashMap<usize, Vec<Delta>>,
    pub file_actor: ActorRef<file_actor::Message>,
    pub stream_processor: StreamProcessor,
    pub reporter: EventReporter,
}
impl ActorState {
    pub async fn new(
        dependency: Dependency,
        cur_context: CurContext,
        file_actor: ActorRef<file_actor::Message>,
    ) -> anyhow::Result<Self> {
        let cur_context_str = cur_context.get_ctx().await;

        let stream_log_path = if dependency.log_streams {
            let path = PathBuf::from(format!(
                "./logs/stream_{}.jsonl",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs()
            ));
            // Ensure the log directory exists
            if let Some(parent) = path.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }
            Some(path)
        } else {
            None
        };

        Ok(Self {
            cur_context,
            history: vec![Message::new(
                "This is the initial context in the environment: \n".to_owned()
                    + cur_context_str.as_str(),
            )],
            llm: dependency.claude,
            tools: dependency.tools,
            acc_map: Default::default(),
            delta_buf: Default::default(),
            stream_actor: None,
            reporter: EventReporter {
                tui_tx: dependency.tui_tx.clone(),
            },
            file_actor,
            stream_processor: StreamProcessor {
                acc_map: Default::default(),
                delta_buf: Default::default(),
                stream_log_path,
                token_count: Default::default(),
                reporter: EventReporter {
                    tui_tx: dependency.tui_tx,
                },
                cur_state: State::Ready,
            },
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
                    reasoning_id,
                } => {
                    self.history.push(Message {
                        role: Role::Assistant,
                        content: vec![ContentBlock::ThinkingBlock {
                            thinking,
                            signature,
                            reasoning_id,
                        }],
                    });
                    Ok(())
                }
                StreamRes::Tool(tool_res) => {
                    let input = tool_res.tool().to_req()?;
                    self.history.push(Message {
                        role: Role::Assistant,
                        content: vec![ContentBlock::ToolBlock {
                            tool_id: tool_res.id(),
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

    async fn tool_use(&self, tool_call: ToolCall) -> anyhow::Result<ToolResult> {
        let tool = tool_call.get_tool()?;
        let ToolCall { id, name, json } = tool_call;
        let res: anyhow::Result<ToolResult> = match tool.clone() {
            Tool::ReadFile(rf) => {
                Tool::ReadFile(rf)
                    .use_tool(id.clone(), &self.cur_context)
                    .await
            }
            Tool::InsertAfterLine(insert) => {
                Tool::InsertAfterLine(insert)
                    .use_tool(id.clone(), &self.cur_context)
                    .await
            }
            Tool::StringReplace(replace) => {
                Tool::StringReplace(replace)
                    .use_tool(id.clone(), &self.cur_context)
                    .await
            }
            Tool::CargoCheck(check) => {
                Tool::CargoCheck(check)
                    .use_tool(id.clone(), &self.cur_context)
                    .await
            }
            Tool::Grep(grep) => {
                Tool::Grep(grep)
                    .use_tool(id.clone(), &self.cur_context)
                    .await
            }
            Tool::CargoTest(test) => {
                Tool::CargoTest(test)
                    .use_tool(id.clone(), &self.cur_context)
                    .await
            }
        };
        match res {
            Ok(res) => Ok(res),
            Err(err) => {
                warn!("{:?}", err.to_string());
                Ok(ToolResult::Error {
                    message: err.to_string(),
                    tool,
                    id: id.clone(),
                })
            }
        }
    }

    pub async fn process_tools(
        &self,
        vec: Vec<PreprocessedStreamItem>,
    ) -> Vec<anyhow::Result<StreamRes>> {
        let futures: Vec<_> = vec
            .into_iter()
            .map(async |item| match item.processed {
                ProcessedItem::String(str) => Ok(StreamRes::String(str.clone())),
                ProcessedItem::Tool(tool) => {
                    let tool_res = self.tool_use(tool).await?;
                    Ok(StreamRes::Tool(tool_res))
                }
                ProcessedItem::Thinking {
                    thinking,
                    signature,
                    reasoning_id,
                } => Ok(StreamRes::Thinking {
                    thinking: thinking.clone(),
                    signature: signature.clone(),
                    reasoning_id: reasoning_id.clone(),
                }),
            })
            .collect();

        future::join_all(futures).await
    }

    pub async fn stream_items_to_res(
        &self,
        vec: Vec<PreprocessedStreamItem>,
    ) -> Vec<anyhow::Result<StreamRes>> {
        let futures: Vec<_> = vec
            .into_iter()
            .map(async |item| match item.processed {
                ProcessedItem::String(str) => Ok(StreamRes::String(str.clone())),
                ProcessedItem::Thinking {
                    thinking,
                    signature,
                    reasoning_id,
                } => Ok(StreamRes::Thinking {
                    thinking: thinking.clone(),
                    signature: signature.clone(),
                    reasoning_id: reasoning_id.clone(),
                }),
                _ => Err(anyhow!("Tool cannot exist for this")),
            })
            .collect();

        future::join_all(futures).await
    }

    pub fn change_state(&mut self, new_state: State) {
        self.stream_processor.change_state(new_state)
    }
}
