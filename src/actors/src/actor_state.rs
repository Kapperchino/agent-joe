use crate::actor::{Dependency, StreamAccu, StreamRes};
use crate::event_reporter::EventReporter;
use crate::file_actor;
use crate::stream_processor::{
    PreprocessedStreamItem, ProcessedItem, StreamAccu, StreamProcessor, ToolCall,
};
use analysis::cur_context::CurContext;
use clients::llm::{ContentBlock, ContentBlockInfo, Delta, LLmClient, Message, Role, StreamEvent};
use clients::tool_defs::{
    CargoCheckInput, InsertAfterLineInput, LenientDeserialize, ReadFileInput, StringReplaceInput,
    Tool, ToolId, ToolResult,
};
use clients::tool_impls;
use common_models::tui_models::ActorToTui;
use common_models::tui_models::State;
use common_models::tui_models::TokenCount;
use futures::{StreamExt, future};
use ractor::{ActorCell, ActorRef};
use std::collections::HashMap;
use std::path::PathBuf;
use anyhow::anyhow;
use tokio::fs::OpenOptions;
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;
use tracing::error;

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

        let stream_log_path = if dependency.save_stream {
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
            file_actor,
            stream_processor: StreamProcessor {
                acc_map: Default::default(),
                delta_buf: Default::default(),
                stream_log_path,
                token_count: Default::default(),
                reporter: EventReporter { tui_tx: () },
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
        let tool = Tool::from_str(tool_call.name.as_str())?;
        let ToolCall { id, name, json } = tool_call;
        let res: anyhow::Result<ToolResult> = match Tool::from_str(name.as_str())? {
            Tool::ReadFile(_) => {
                let input = ReadFileInput::deserialize_lenient(&json)?;
                let rf = clients::tool_defs::ReadFile {
                    id: id.id.clone(),
                    input,
                };
                Ok(Tool::ReadFile(rf)
                    .use_tool(id.clone(), &self.cur_context)
                    .await?)
            }
            Tool::InsertAfterLine(_) => {
                let input = InsertAfterLineInput::deserialize_lenient(&json)?;
                let rf = clients::tool_defs::InsertAfterLine {
                    id: id.id.clone(),
                    input,
                };
                Ok(Tool::InsertAfterLine(rf)
                    .use_tool(id.clone(), &self.cur_context)
                    .await?)
            }
            Tool::StringReplace(_) => {
                let input = StringReplaceInput::deserialize_lenient(&json)?;
                let rf = clients::tool_defs::StringReplace {
                    id: id.id.clone(),
                    input,
                };
                Ok(Tool::StringReplace(rf)
                    .use_tool(id.clone(), &self.cur_context)
                    .await?)
            }
            Tool::CargoCheck(_) => {
                let input = if json.is_empty() {
                    CargoCheckInput {
                        include_warnings: None,
                    }
                } else {
                    CargoCheckInput::deserialize_lenient(&json)?
                };
                let rf = clients::tool_defs::CargoCheck {
                    id: id.id.clone(),
                    input,
                };
                Ok(Tool::CargoCheck(rf)
                    .use_tool(id.clone(), &self.cur_context)
                    .await?)
            }
        };
        match res {
            Ok(res) => Ok(res),
            Err(err) => Ok(ToolResult::Error {
                message: err.to_string(),
                tool,
                id: id.clone(),
            }),
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
                _ => {Err(anyhow!("Tool cannot exist for this"))}
            })
            .collect();

        future::join_all(futures).await
    }

    pub fn change_state(&mut self, new_state: State) {
        self.stream_processor.change_state(new_state)
    }
}
