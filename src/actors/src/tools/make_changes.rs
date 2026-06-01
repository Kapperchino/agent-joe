use crate::actor::{ActorContext, Dependency, Message};
use crate::worker::{Worker, WorkerAdapter};
use crate::workers::write_worker::WriteWorker;
use analysis::contexts::rust_context::RustContext;
use analysis::contexts::rust_empty_context::RustEmptyContext;
use anyhow::anyhow;
use async_trait::async_trait;
use ractor::{Actor, call};
use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};
use tools::tool_defs::{ToolDefTrait, ToolId, ToolTrait, ToolType};
use turbo_code_macros::{ToolDef, ToolInput};

#[async_trait]
impl ToolTrait<RustContext, ActorContext<RustContext>> for MakeChanges {
    type Input = MakeChangesInput;
    type Output = MakeChangesResult;

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
        let init_prompt = format!("You are a write enabled agent in a rust codebase given a prompt to modify the codebase\
        follow the instructions, and write the code in idiomatic rust and follow the coding style of the surrounding code. After \
        the changes are made and validated, respond with the changes made to the orchestrator.
         \n{question}").to_owned();
        cur_context.initial_prompt = init_prompt;

        let empty_context = RustEmptyContext::new(cur_context, true);

        let (joe, actor_handle) = Actor::spawn_linked(
            None,
            WorkerAdapter::new(WriteWorker::new()),
            Dependency {
                client: info.dep.client.clone(),
                tools: WriteWorker::tools(),
                tui_tx: info.dep.tui_tx.clone(),
                debug_mode: info.dep.debug_mode.clone(),
                context: empty_context,
            },
            info.actor_ref.get_cell(),
        )
        .await?;
        joe.send_message(Message::StartWork(None))?;
        let res = call!(joe, |reply| {
            Message::RegisterCallback(info.actor_ref.get_id(), reply)
        })?;

        Ok(MakeChangesResult { res, id: tool_id })
    }

    fn display_input(input: &Self::Input) -> String {
        MakeChanges {
            input: input.clone(),
            id: String::new(),
        }
        .to_string()
    }

    fn req_from_input(
        input: &Self::Input,
    ) -> anyhow::Result<std::collections::HashMap<String, String>> {
        MakeChanges {
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

    fn tool_type() -> ToolType {
        ToolType::Client
    }
}

#[derive(Default, Serialize, Deserialize, Debug, Clone, ToolDef)]
#[tool(name = "make_changes", description = "Create an agent to make changes")]
pub struct MakeChanges {
    #[tool(input)]
    pub input: MakeChangesInput,
    pub id: String,
}

#[derive(Default, Serialize, Deserialize, Debug, Clone, ToolInput)]
pub struct MakeChangesInput {
    #[tool(description = "Context to give to the agent", required)]
    pub context: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MakeChangesResult {
    pub res: String,
    pub id: ToolId,
}

impl Display for MakeChanges {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let context = self.input.context.trim();
        if context.is_empty() {
            write!(f, "- make changes")
        } else {
            let summary = context.lines().next().unwrap_or(context).trim();
            let summary: String = summary.chars().take(80).collect();
            if context.chars().count() > 80 {
                write!(f, "- make changes: `{summary}...`")
            } else {
                write!(f, "- make changes: `{summary}`")
            }
        }
    }
}
