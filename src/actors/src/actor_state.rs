use crate::actor;
use crate::actor::{ActorContext, ActorInfo, Dependency, StreamRes};
use crate::background_actors::file_actor;
use crate::event_reporter::EventReporter;
use crate::stream_processor::{PreprocessedStreamItem, ProcessedItem, StreamProcessor};
use crate::tool_call::ToolCall;
use analysis::contexts::context::Context;
use anyhow::anyhow;
use clients::llm::{ContentBlock, LLmClient, Message, Role};
use common_models::tui_models::State;
use futures::future;
use ractor::{ActorCell, ActorId, ActorRef, RpcReplyPort};
use std::collections::HashMap;
use std::path::PathBuf;
use tools::tool_defs::{ErasedToolRef, ToolDefinition, ToolInvocation, ToolResult};
use tracing::{error, warn};
use utils::utils::FnvHashMap;

pub struct ActorState<C: Context> {
    pub cur_context: C,
    pub stream_actor: Option<ActorCell>,
    pub history: Vec<Message>,
    pub llm: LLmClient,
    pub tools: FnvHashMap<String, ErasedToolRef<C, ActorContext<C>>>,
    pub file_actor: Option<ActorRef<file_actor::Message>>,
    pub pending_ports: FnvHashMap<ActorId, RpcReplyPort<String>>,
    pub stream_processor: StreamProcessor,
    pub reporter: EventReporter,
    pub debug_mode: bool,
    pub actor_ref: ActorRef<actor::Message>,
    dependency: Dependency<C>,
}
impl<C: Context + Clone> ActorState<C> {
    pub async fn new(
        dependency: Dependency<C>,
        actor_ref: ActorRef<actor::Message>,
        file_actor: Option<ActorRef<file_actor::Message>>,
    ) -> anyhow::Result<Self> {
        let dep_clone = dependency.clone();
        let cur_context_str = dependency.context.get_ctx().await;

        let stream_log_path = if dependency.debug_mode {
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
            cur_context: dependency.context,
            history: vec![Message::new(cur_context_str)],
            llm: dependency.client,
            tools: dependency
                .tools
                .into_iter()
                .map(|x| (x.name(), x))
                .collect(),
            stream_actor: None,
            reporter: EventReporter {
                actor_id: actor_ref.get_id(),
                tui_tx: dependency.tui_tx.clone(),
            },
            debug_mode: dependency.debug_mode,
            file_actor,
            pending_ports: FnvHashMap::default(),
            stream_processor: StreamProcessor {
                batches: vec![],
                stream_log_path,
                token_count: Default::default(),
                reporter: EventReporter {
                    actor_id: actor_ref.get_id(),
                    tui_tx: dependency.tui_tx,
                },
                cur_state: State::Ready,
                debug: dependency.debug_mode,
            },
            dependency: dep_clone,
            actor_ref,
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
                    let tool = self.find_tool(&tool_res.invocation.name)?;
                    tool.add_context(
                        &tool_res.invocation.input,
                        &mut self.cur_context,
                        &tool_res.content,
                    )?;
                    self.history.push(Message {
                        role: Role::Assistant,
                        content: vec![ContentBlock::ToolBlock {
                            tool_id: tool_res.id(),
                            name: tool_res.invocation.name.clone(),
                            input: tool.input_req_erased(&tool_res.invocation.input)?,
                        }],
                    });
                    self.history.push(Message {
                        role: Role::User,
                        content: vec![Self::tool_res_to_json(tool_res)],
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

    pub fn tool_definitions(&self) -> Vec<ToolDefinition> {
        self.tools.values().map(|tool| tool.definition()).collect()
    }

    fn find_tool(&self, name: &str) -> anyhow::Result<ErasedToolRef<C, ActorContext<C>>> {
        Ok(self
            .tools
            .get(name)
            .ok_or_else(|| anyhow!("unknown tool `{}`", name))
            .cloned()?)
    }

    pub fn tool_display(&self, tool_call: &ToolCall) -> anyhow::Result<String> {
        let tool = self.find_tool(&tool_call.name)?;
        let input = tool_call.input_value()?;
        tool.display_erased(&input)
    }

    async fn tool_use(&self, tool_call: ToolCall) -> anyhow::Result<ToolResult> {
        let tool = self.find_tool(&tool_call.name)?;
        let tool_name = tool_call.name.clone();
        let id = tool_call.id.clone();
        let input = tool_call.input_value()?;

        if self.debug_mode {
            warn!(tool_name = %tool_name, tool_id = ?id, input = ?input, "tool input");
        }

        let invocation = ToolInvocation {
            name: tool_name.clone(),
            input: input.clone(),
            display: tool.display_erased(&input)?,
        };

        match tool
            .run_erased(
                input.clone(),
                id.clone(),
                &self.cur_context,
                &ActorContext::ActorInfo(ActorInfo {
                    dep: self.dependency.clone(),
                    actor_ref: self.actor_ref.clone(),
                }),
            )
            .await
        {
            Ok(output) => {
                let content = tool.output_to_content_erased(&input, &output)?;
                if self.debug_mode {
                    warn!(
                        tool_name = %tool_name,
                        tool_id = ?id,
                        output = ?output,
                        content = %content,
                        "tool output"
                    );
                }
                Ok(ToolResult::success(id, invocation, content))
            }
            Err(err) => {
                if self.debug_mode {
                    warn!(tool_name = %tool_name, tool_id = ?id, error = ?err, "tool error");
                }
                warn!("{:?}", err.to_string());
                Ok(ToolResult::error(id, invocation, err.to_string()))
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

    pub fn tool_res_to_json(res: ToolResult) -> ContentBlock {
        ContentBlock::ToolResult {
            tool_id: res.id,
            content: res.content,
            is_error: Some(res.is_error),
        }
    }
}
