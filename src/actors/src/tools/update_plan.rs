use crate::actor::{ActorContext, Message};
use crate::runtime::ExecutionRole;
use analysis::contexts::context::Context;
use async_trait::async_trait;
use common_models::interaction::PlanUpdate;
use serde_json::{Value, json};
use tools::tool_defs::{
    LenientDeserialize, ToolDefTrait, ToolEffect, ToolId, ToolProperty, ToolTrait, ToolType,
};
use utils::utils::FnvHashMap;

pub struct UpdatePlan;

#[derive(Clone, serde::Deserialize)]
#[serde(transparent)]
pub struct Input {
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

impl ToolDefTrait for UpdatePlan {
    fn tool_name() -> &'static str {
        "update_plan"
    }
    fn tool_description() -> &'static str {
        "Persist 1–16 plan steps with stable IDs, dependencies, acceptance criteria and states. Use the current plan and requirements revisions from runtime context. A new step may start in_progress if its dependencies are completed; at most one step may be in_progress. Start steps before completing; completion needs successful evidence source IDs from runtime context plus explanations. After requirements change, reopen completed steps before revalidating. Does not change work mode."
    }
    fn field_properties() -> FnvHashMap<String, ToolProperty> {
        json!({
            "revision":{"type":"integer","description":"Current plan revision"},
            "requirements_revision":{"type":"integer","description":"Current requirements revision"},
            "steps":{"type":"array","minItems":1,"maxItems":16,"items":{"type":"object","additionalProperties":false,"properties":{
                "id":{"type":"string"},"description":{"type":"string"},"dependencies":{"type":"array","items":{"type":"string"}},
                "acceptance":{"type":"string"},"state":{"type":"string","enum":["pending","in_progress","completed","blocked"]},
                "evidence":{"type":"array","items":{"type":"object","additionalProperties":false,"properties":{"source":{"type":"string"},"explanation":{"type":"string"}},"required":["source","explanation"]}},
                "blocked_reason":{"type":["string","null"]}
            },"required":["id","description","dependencies","acceptance","state","evidence","blocked_reason"]}}
}).as_object().unwrap().iter()
            .map(|(name, schema)| (name.clone(), ToolProperty::Schema(schema.clone()))).collect()
    }
    fn required_fields() -> Vec<String> {
        ["revision", "requirements_revision", "steps"]
            .into_iter()
            .map(str::to_owned)
            .collect()
    }
    fn req(&self) -> anyhow::Result<FnvHashMap<String, String>> {
        Ok(Default::default())
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
        let info = match actor {
            ActorContext::ActorInfo(info)
                if matches!(info.dep.runtime.role, ExecutionRole::Root) =>
            {
                Ok(info)
            }
            _ => Err(anyhow::anyhow!("User interaction requires the root actor")),
        }?;
        let (reply, receive) = tokio::sync::oneshot::channel();
        info.actor_ref
            .send_message(Message::UpdatePlan {
                update: input.update,
                scope: crate::actor::InteractionScope {
                    execution: info.dep.runtime.scope.clone(),
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
    fn effect() -> ToolEffect {
        ToolEffect::Interaction
    }
    fn tool_type() -> ToolType {
        ToolType::Client
    }
}
