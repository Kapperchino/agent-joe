use crate::actor::{ActorContext, Dependency};
use crate::worker::Worker;
use crate::workers::read_worker::ReadWorker;
use analysis::contexts::context::Context;
use analysis::contexts::rust_context::RustContext;
use analysis::contexts::rust_empty_context::RustEmptyContext;
use anyhow::anyhow;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};
use tools::tool_defs::{ToolDefTrait, ToolId, ToolTrait, ToolType};
use turbo_code_macros::{ToolDef, ToolInput};
use utils::utils::FnvHashMap;

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

        let mut new_context = cur_context.clone();
        new_context.initial_prompt = ReadWorker::init_prompt(None);
        new_context.task_prompt = Some(input.context.clone());

        let empty_context = RustEmptyContext::new(new_context, true, cur_context.gen_id());

        crate::worker::run_worker(
            ReadWorker::new(),
            Dependency {
                client: info.dep.client.clone(),
                tools: ReadWorker::tools(),
                tui_tx: info.dep.tui_tx.clone(),
                debug_mode: info.dep.debug_mode,
                context: empty_context,
                runtime: info.dep.runtime.child(info.dep.runtime.scope.child()),
            },
            info.actor_ref.clone(),
        )
        .await
        .map_err(|error| error.into_tool_failure().into())
        .map(|res| GatherContextResult { res, id: tool_id })
    }

    fn display_input(input: &Self::Input) -> String {
        GatherContext {
            input: input.clone(),
            id: String::new(),
        }
        .to_string()
    }

    fn req_from_input(input: &Self::Input) -> anyhow::Result<FnvHashMap<String, String>> {
        GatherContext {
            input: input.clone(),
            id: String::new(),
        }
        .req()
    }

    fn output_to_content(_input: &Self::Input, output: &Self::Output) -> anyhow::Result<String> {
        Ok(output.res.clone())
    }

    fn add_context(input: &Self::Input, context: &mut RustContext, addition: &str) {
        context
            .stacked_context
            .push(format!("{}\n{addition}", input.context))
    }

    fn effect() -> tools::tool_defs::ToolEffect {
        tools::tool_defs::ToolEffect::DelegateRead
    }

    fn tool_type() -> ToolType {
        ToolType::Client
    }
}

#[derive(Default, Serialize, Deserialize, Debug, Clone, ToolDef)]
#[tool(
    name = "gather_context",
    description = "Create an agent to gather context, make sure to keep the scope small and parallelize if needed,\
    give the agent all the context it needs such as file paths, symbols that are relevant and all the context that are related."
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
