use crate::actor::{ActorContext, Message};
use crate::states::runtime::ExecutionRole;
use analysis::contexts::context::Context;
use async_trait::async_trait;
use common_models::interaction::PlanUpdate;
use serde_json::Value;
use tools::tool_defs::{LenientDeserialize, ToolId, ToolOpKind, ToolTrait, ToolType};
use turbo_code_macros::{ToolDef, ToolSchema};
use utils::utils::FnvHashMap;

#[derive(ToolDef)]
#[tool(
    name = "update_plan",
    description = "Track work with 1–16 stable steps, dependencies and acceptance criteria. Plan mode requires completed investigation steps with observed evidence before finishing; future implementation steps may remain pending. In implementation mode, skip simple tasks unless specific validation was requested. Record each requested Cargo check in an implementation step's validation field using its exact Cargo tool parameters. Implementation completion requires those checks to succeed on the current workspace. Batch progress into one update at meaningful milestones. Use current revisions from runtime context. Existing pending steps may complete directly with successful evidence source IDs and explanations; dependencies must be completed. New steps start pending or in_progress; at most one is in_progress. Reopen completed steps after requirements change and changed or blocked steps before completion. Does not change work mode.",
    input = "Input"
)]
pub struct UpdatePlan;

#[derive(Clone, serde::Deserialize, ToolSchema)]
#[serde(transparent)]
pub struct Input {
    #[tool(schema = "PlanUpdate<tools::cargo_tools::CargoRequest>")]
    update: PlanUpdate,
}

impl LenientDeserialize for Input {
    fn deserialize_lenient(value: Value) -> anyhow::Result<Self> {
        Ok(serde_json::from_value(value)?)
    }
}

impl std::fmt::Display for UpdatePlan {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "update_plan")
    }
}

#[async_trait]
impl<C: Context> ToolTrait<C, ActorContext<C>> for UpdatePlan {
    type Input = Input;
    type Output = String;

    async fn run(
        input: Self::Input,
        _: ToolId,
        _: &C,
        actor: &ActorContext<C>,
    ) -> anyhow::Result<String> {
        for validation in input
            .update
            .steps
            .iter()
            .filter_map(|step| step.validation.as_ref())
        {
            serde_json::from_value::<tools::cargo_tools::CargoRequest>(serde_json::Value::Object(
                validation.cargo.clone(),
            ))?
            .validation_command()?;
        }
        let info = match actor {
            ActorContext::ActorInfo(info) if matches!(info.runtime.role, ExecutionRole::Root) => {
                Ok(info)
            }
            _ => Err(anyhow::anyhow!("User interaction requires the root actor")),
        }?;
        let (reply, receive) = tokio::sync::oneshot::channel();
        info.actor_ref
            .send_message(Message::UpdatePlan {
                update: input.update,
                scope: crate::actor::InteractionScope {
                    execution: info.runtime.scope.clone(),
                },
                reply: reply.into(),
            })
            .map_err(|error| anyhow::anyhow!(error.to_string()))?;
        receive.await?.map_err(anyhow::Error::msg)
    }

    fn display_input(_: &Self::Input) -> String {
        "update_plan".into()
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
