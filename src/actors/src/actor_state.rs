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
use std::path::PathBuf;
use tools::tool_defs::{ErasedToolRef, ToolDefinition, ToolInvocation, ToolResult};
use tracing::warn;
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
        let history = Self::initial_history(&dependency.context).await;

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

        let context = dependency.context;
        let actor_id = context.get_id();

        let reporter = EventReporter {
            actor_id,
            tui_tx: dependency.tui_tx.clone(),
        };

        Ok(Self {
            cur_context: context,
            history,
            llm: dependency.client,
            tools: dependency
                .tools
                .into_iter()
                .map(|x| (x.name(), x))
                .collect(),
            stream_actor: None,
            reporter: reporter.clone(),
            debug_mode: dependency.debug_mode,
            file_actor,
            pending_ports: FnvHashMap::default(),
            stream_processor: StreamProcessor {
                batches: vec![],
                stream_log_path,
                token_count: Default::default(),
                reporter,
                cur_state: State::Ready,
                debug: dependency.debug_mode,
            },
            dependency: dep_clone,
            actor_ref,
        })
    }

    async fn initial_history(context: &C) -> Vec<Message> {
        let mut history = vec![Message::new(context.get_ctx().await)];
        if let Some(task) = context.initial_task() {
            history.push(Message::new(task.to_owned()));
        }
        history
    }

    pub fn build_request(&self) -> clients::llm::ClientRequest {
        clients::llm::ClientRequest::new(self.history.clone())
            .with_system(self.cur_context.instructions().to_owned())
            .with_tools(self.tool_definitions())
            .with_thinking()
    }

    pub async fn clear_history(&mut self) {
        self.cur_context.clear_task_context();
        self.history = Self::initial_history(&self.cur_context).await;
    }

    pub fn save_history(&mut self, results: Vec<anyhow::Result<StreamRes>>) -> anyhow::Result<()> {
        let results = results.into_iter().collect::<anyhow::Result<Vec<_>>>()?;
        let mut assistant = Vec::new();
        let mut outputs = Vec::new();
        for result in results {
            match result {
                StreamRes::Content(content) => assistant.push(content),
                StreamRes::Tool(result) => {
                    let (id, name, input) = match &result {
                        ToolResult::Success {
                            id,
                            invocation,
                            content,
                        } => {
                            self.find_tool(&invocation.name)?.add_context(
                                &invocation.input,
                                &mut self.cur_context,
                                content,
                            )?;
                            (id, &invocation.name, &invocation.input)
                        }
                        ToolResult::Failure {
                            id, name, input, ..
                        } => (id, name, input),
                    };
                    assistant.push(ContentBlock::ToolBlock {
                        tool_id: id.clone(),
                        name: name.clone(),
                        input: input.clone(),
                    });
                    outputs.push(Self::tool_res_to_json(&result));
                }
            }
        }
        // Keep the provider's entire assistant response in order, then return
        // every tool result together (also required by Claude's message format).
        if !assistant.is_empty() {
            self.history.push(Message {
                role: Role::Assistant,
                content: assistant,
            });
        }
        if !outputs.is_empty() {
            self.history.push(Message {
                role: Role::User,
                content: outputs,
            });
        }
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
                Ok(ToolResult::error(
                    id,
                    tool_name,
                    input.clone(),
                    err.to_string(),
                ))
            }
        }
    }

    pub async fn process_tools(
        &self,
        items: Vec<PreprocessedStreamItem>,
    ) -> Vec<anyhow::Result<StreamRes>> {
        future::join_all(items.into_iter().map(async |item| match item.processed {
            ProcessedItem::Content(content) => Ok(StreamRes::Content(content)),
            ProcessedItem::Tool(tool) => {
                let result = match self.tool_use(tool.clone()).await {
                    Ok(result) => result,
                    Err(err) => ToolResult::error(
                        tool.id,
                        tool.name,
                        serde_json::from_str(&tool.json)?,
                        err.to_string(),
                    ),
                };
                Ok(StreamRes::Tool(result))
            }
        }))
        .await
    }

    pub async fn stream_items_to_res(
        &self,
        items: Vec<PreprocessedStreamItem>,
    ) -> Vec<anyhow::Result<StreamRes>> {
        items
            .into_iter()
            .map(|item| match item.processed {
                ProcessedItem::Content(content) => Ok(StreamRes::Content(content)),
                ProcessedItem::Tool(_) => Err(anyhow!("Unexpected tool in a completed response")),
            })
            .collect()
    }

    pub fn change_state(&mut self, new_state: State) {
        self.stream_processor.change_state(new_state)
    }

    pub fn tool_res_to_json(res: &ToolResult) -> ContentBlock {
        ContentBlock::ToolResult {
            tool_id: res.id(),
            content: match &res {
                ToolResult::Success { content, .. } => content.clone(),
                ToolResult::Failure { msg, .. } => msg.clone(),
            },
            is_error: match &res {
                ToolResult::Success { .. } => None,
                ToolResult::Failure { .. } => Some(true),
            },
        }
    }
}
