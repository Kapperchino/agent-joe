use crate::actor::{ActorContext, Message};
use crate::states::runtime::ExecutionRole;
use analysis::contexts::context::Context;
use async_trait::async_trait;
use common_models::interaction::Question;
use serde_json::Value;
use tools::tool_defs::{LenientDeserialize, ToolId, ToolOpKind, ToolTrait, ToolType};
use turbo_code_macros::{ToolDef, ToolSchema};
use utils::utils::FnvHashMap;

#[derive(ToolDef)]
#[tool(
    name = "request_user_input",
    description = "Ask a structured clarification question with an unused ID, choices and/or free text. Required questions pause continuation after current work is cleaned up; optional questions allow independent work. Only the user can answer. Answers cannot widen workspace permissions.",
    input = "Input"
)]
pub struct RequestUserInput;

#[derive(Clone, serde::Deserialize, ToolSchema)]
#[serde(transparent)]
pub struct Input {
    question: Question,
}

impl LenientDeserialize for Input {
    fn deserialize_lenient(value: Value) -> anyhow::Result<Self> {
        Ok(serde_json::from_value(value)?)
    }
}

impl std::fmt::Display for RequestUserInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "request_user_input")
    }
}

#[async_trait]
impl<C: Context> ToolTrait<C, ActorContext<C>> for RequestUserInput {
    type Input = Input;
    type Output = String;

    async fn run(
        input: Self::Input,
        _: ToolId,
        _: &C,
        actor: &ActorContext<C>,
    ) -> anyhow::Result<String> {
        let info = match actor {
            ActorContext::ActorInfo(info) if matches!(info.runtime.role, ExecutionRole::Root) => {
                Ok(info)
            }
            _ => Err(anyhow::anyhow!("User interaction requires the root actor")),
        }?;
        let (reply, receive) = tokio::sync::oneshot::channel();
        info.actor_ref
            .send_message(Message::AskQuestion {
                question: input.question,
                scope: crate::actor::InteractionScope {
                    execution: info.runtime.scope.clone(),
                },
                reply: reply.into(),
            })
            .map_err(|error| anyhow::anyhow!(error.to_string()))?;
        receive.await?.map_err(anyhow::Error::msg)
    }

    fn display_input(_: &Self::Input) -> String {
        "request_user_input".into()
    }
    fn req_from_input(_: &Self::Input) -> anyhow::Result<FnvHashMap<String, String>> {
        Ok(Default::default())
    }
    fn output_to_content(_: &Self::Input, output: &String) -> anyhow::Result<String> {
        Ok(output.clone())
    }
    fn effect() -> ToolOpKind {
        ToolOpKind::Interaction
    }
    fn tool_type() -> ToolType {
        ToolType::Client
    }
}
