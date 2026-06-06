use crate::actor::{ActorContext, Dependency, Message};
use crate::worker::{Worker, WorkerAdapter};
use crate::workers::validate_worker::ValidateWorker;
use analysis::contexts::rust_empty_context::RustEmptyContext;
use anyhow::anyhow;
use async_trait::async_trait;
use ractor::{Actor, call};
use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};
use tools::tool_defs::{ToolDefTrait, ToolId, ToolTrait, ToolType};
use turbo_code_macros::{ToolDef, ToolInput};
use utils::utils::FnvHashMap;

#[async_trait]
impl ToolTrait<RustEmptyContext, ActorContext<RustEmptyContext>> for ValidateRust {
    type Input = ValidateRustInput;
    type Output = ValidateRustResult;

    async fn run(
        input: Self::Input,
        tool_id: ToolId,
        cur_context: &RustEmptyContext,
        actor_context: &ActorContext<RustEmptyContext>,
    ) -> anyhow::Result<Self::Output> {
        let info = match actor_context {
            ActorContext::ActorInfo(info) => Ok(info),
            _ => Err(anyhow!("wrong actor context")),
        }?;

        let mut cur_context = cur_context.clone();
        cur_context.inner.initial_prompt = ValidateWorker::init_prompt(Some(&input.context));
        cur_context.stack_context = false;

        let (joe, actor_handle) = Actor::spawn_linked(
            None,
            WorkerAdapter::new(ValidateWorker::new()),
            Dependency {
                client: info.dep.client.clone(),
                tools: ValidateWorker::tools(),
                tui_tx: info.dep.tui_tx.clone(),
                debug_mode: info.dep.debug_mode.clone(),
                context: cur_context,
            },
            info.actor_ref.get_cell(),
        )
        .await?;
        joe.send_message(Message::StartWork(None))?;
        let res = call!(joe, |reply| {
            Message::RegisterCallback(info.actor_ref.get_id(), reply)
        })?;

        Ok(ValidateRustResult { res, id: tool_id })
    }

    fn display_input(input: &Self::Input) -> String {
        ValidateRust {
            input: input.clone(),
            id: String::new(),
        }
        .to_string()
    }

    fn req_from_input(input: &Self::Input) -> anyhow::Result<FnvHashMap<String, String>> {
        ValidateRust {
            input: input.clone(),
            id: String::new(),
        }
        .req()
    }

    fn output_to_content(_input: &Self::Input, output: &Self::Output) -> anyhow::Result<String> {
        Ok(output.res.clone())
    }

    fn tool_type() -> ToolType {
        ToolType::Client
    }
}

#[derive(Default, Serialize, Deserialize, Debug, Clone, ToolDef)]
#[tool(
    name = "validate_rust",
    description = "Create an agent to validate a rust workplace"
)]
pub struct ValidateRust {
    #[tool(input)]
    pub input: ValidateRustInput,
    pub id: String,
}

#[derive(Default, Serialize, Deserialize, Debug, Clone, ToolInput)]
pub struct ValidateRustInput {
    #[tool(description = "Context to give to the agent", required)]
    pub context: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidateRustResult {
    pub res: String,
    pub id: ToolId,
}

impl Display for ValidateRust {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let context = self.input.context.trim();
        if context.is_empty() {
            write!(f, "- validate rust")
        } else {
            let summary = context.lines().next().unwrap_or(context).trim();
            let summary: String = summary.chars().take(80).collect();
            if context.chars().count() > 80 {
                write!(f, "- validate rust: `{summary}...`")
            } else {
                write!(f, "- validate rust: `{summary}`")
            }
        }
    }
}
