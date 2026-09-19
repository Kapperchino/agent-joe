use crate::{
    actor::ActorContext,
    immutable_workers::{ImmutableAnswer, ImmutableWorkerView},
};
use analysis::contexts::context::Context;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tools::tool_defs::{ToolDefTrait, ToolEffect, ToolId, ToolTrait, ToolType};
use turbo_code_macros::{ToolDef, ToolInput};
use utils::utils::FnvHashMap;

#[derive(Default, Debug, Clone, Serialize, Deserialize, ToolDef)]
#[tool(
    name = "ask_immutable_worker",
    description = "List immutable workers available in this conversation or ask one a question by worker_id. List returns each worker's kind and description. Workers answer independently from fixed context, without tools or retaining questions and answers. Compaction automatically creates snapshot workers preserving older context; other immutable worker kinds use the same interface. Workers are in-memory and unavailable after clear, session switch, or shutdown."
)]
pub struct AskImmutableWorker {
    #[tool(input)]
    pub input: AskImmutableWorkerInput,
}

#[derive(Default, Debug, Clone, Serialize, Deserialize, ToolInput)]
pub struct AskImmutableWorkerInput {
    #[tool(description = "list or ask", required)]
    pub action: String,
    #[tool(description = "Immutable worker ID from list; required for ask")]
    pub worker_id: Option<String>,
    #[tool(description = "Independent question, up to 16384 UTF-8 bytes; required for ask")]
    pub question: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
enum Action {
    List,
    Ask { worker_id: String, question: String },
}

impl TryFrom<AskImmutableWorkerInput> for Action {
    type Error = anyhow::Error;

    fn try_from(input: AskImmutableWorkerInput) -> anyhow::Result<Self> {
        match input.action.as_str() {
            "list" => match input {
                AskImmutableWorkerInput {
                    worker_id: None,
                    question: None,
                    ..
                } => Ok(Self::List),
                _ => Err(anyhow::anyhow!("Use list without worker_id or question")),
            },
            "ask" => Ok(Self::Ask {
                worker_id: input
                    .worker_id
                    .filter(|worker_id| !worker_id.trim().is_empty())
                    .ok_or_else(|| anyhow::anyhow!("Use ask with a nonempty worker_id"))?,
                question: input
                    .question
                    .filter(|question| !question.trim().is_empty())
                    .filter(|question| question.len() <= 16384)
                    .ok_or_else(|| {
                        anyhow::anyhow!("Use ask with a nonempty question of at most 16384 bytes")
                    })?,
            }),
            _ => Err(anyhow::anyhow!(
                "Use list without worker_id or question, or ask with a worker_id and a nonempty question of at most 16384 bytes"
            )),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum AskImmutableWorkerOutput {
    List { workers: Vec<ImmutableWorkerView> },
    Ask { result: ImmutableAnswer },
}

impl std::fmt::Display for AskImmutableWorker {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "Immutable worker {} {}",
            self.input.action,
            self.input.worker_id.as_deref().unwrap_or_default()
        )
    }
}

#[async_trait]
impl<C: Context> ToolTrait<C, ActorContext<C>> for AskImmutableWorker {
    type Input = AskImmutableWorkerInput;
    type Output = AskImmutableWorkerOutput;

    async fn run(
        input: Self::Input,
        _: ToolId,
        _: &C,
        actor: &ActorContext<C>,
    ) -> anyhow::Result<Self::Output> {
        let action = Action::try_from(input)?;
        let info = match actor {
            ActorContext::ActorInfo(info) => Ok(info),
            ActorContext::Noop => Err(anyhow::anyhow!("Immutable workers require a conversation")),
        }?;
        let registry = &info.dep.runtime.immutable_workers;
        let owner = info.dep.worker_owner();
        match action {
            Action::List => Ok(Self::Output::List {
                workers: registry.list(&owner),
            }),
            Action::Ask {
                worker_id,
                question,
            } => Ok(Self::Output::Ask {
                result: registry
                    .ask(
                        &owner,
                        &worker_id,
                        question,
                        info.dep.runtime.request_timeout,
                    )
                    .await?,
            }),
        }
    }

    fn display_input(input: &Self::Input) -> String {
        Self {
            input: input.clone(),
        }
        .to_string()
    }

    fn req_from_input(input: &Self::Input) -> anyhow::Result<FnvHashMap<String, String>> {
        Self {
            input: input.clone(),
        }
        .req()
    }

    fn output_to_content(_: &Self::Input, output: &Self::Output) -> anyhow::Result<String> {
        Ok(serde_json::to_string(output)?)
    }

    fn effect() -> ToolEffect {
        ToolEffect::DelegateRead
    }

    fn tool_type() -> ToolType {
        ToolType::Client
    }
}

#[cfg(test)]
#[path = "../../tests/unit/ask_immutable_worker/tests.rs"]
mod tests;
