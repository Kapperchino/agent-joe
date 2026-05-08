use crate::actor::{ActorContext, Dependency};
use crate::worker::{Worker, WorkerAdapter};
use crate::workers::read_worker::ReadWorker;
use analysis::contexts::rust_context::RustContext;
use anyhow::anyhow;
use async_trait::async_trait;
use ractor::Actor;
use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};
use tools::tool_defs::{ToolDefTrait, ToolId, ToolTrait};
use turbo_code_macros::{ToolDef, ToolInput};

#[async_trait]
impl ToolTrait<RustContext, ActorContext<RustContext>> for GatherContext {
    type Input = GatherContextInput;
    type Output = GatherContextResult;

    async fn run(
        input: Self::Input,
        tool_id: ToolId,
        cur_context: &RustContext,
        actor_context: &ActorContext<RustContext>,
    ) -> anyhow::Result<Self::Output> {
        let info = match actor_context {
            ActorContext::ActorInfo(info) => Ok(info),
            _ => Err(anyhow!("wrong actor context")),
        }?;

        let question = input.context;
        let mut cur_context = cur_context.clone();
        let init_prompt = format!("You are read-only agent in a rust code base,\
     you will be asked a general question by the parent agent, make sure you thoroughly explore the \
     codebase before you give your final answer\n{question}").to_owned();
        cur_context.initial_prompt = init_prompt;

        let (joe, actor_handle) = Actor::spawn_linked(
            None,
            WorkerAdapter::new(ReadWorker::new()),
            Dependency {
                client: info.dep.client.clone(),
                tools: ReadWorker::tools(),
                tui_tx: info.dep.tui_tx.clone(),
                debug_mode: info.dep.debug_mode.clone(),
                context: cur_context,
            },
            info.actor_ref.get_cell(),
        )
        .await
        .expect("Failed to start actor");
        anyhow::bail!("gather_context tool is not implemented")
    }

    fn display_input(input: &Self::Input) -> String {
        GatherContext {
            input: input.clone(),
            id: String::new(),
        }
        .to_string()
    }

    fn req_from_input(
        input: &Self::Input,
    ) -> anyhow::Result<std::collections::HashMap<String, String>> {
        GatherContext {
            input: input.clone(),
            id: String::new(),
        }
        .req()
    }

    fn output_to_content(_input: &Self::Input, output: &Self::Output) -> anyhow::Result<String> {
        Ok(output.res.clone())
    }
}

#[derive(Default, Serialize, Deserialize, Debug, Clone, ToolDef)]
#[tool(
    name = "gather_context",
    description = "Create an agent to gather context"
)]
pub struct GatherContext {
    #[tool(input)]
    pub input: GatherContextInput,
    pub id: String,
}

#[derive(Default, Serialize, Deserialize, Debug, Clone, ToolInput)]
pub struct GatherContextInput {
    #[tool(description = "Context to give to the agent", required)]
    pub context: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatherContextResult {
    pub res: String,
    pub id: ToolId,
}

impl Display for GatherContext {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let context = self.input.context.trim();
        if context.is_empty() {
            write!(f, "- gather context")
        } else {
            let summary = context.lines().next().unwrap_or(context).trim();
            let summary: String = summary.chars().take(80).collect();
            if context.chars().count() > 80 {
                write!(f, "- gather context: `{summary}...`")
            } else {
                write!(f, "- gather context: `{summary}`")
            }
        }
    }
}
