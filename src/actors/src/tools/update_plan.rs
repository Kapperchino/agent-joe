use crate::actor::{ActorContext, Message};
use crate::states::runtime::ExecutionRole;
use analysis::contexts::context::Context;
use async_trait::async_trait;
use common_models::interaction::PlanUpdate;
use serde_json::{Value, json};
use tools::tool_defs::{
    LenientDeserialize, ToolDefTrait, ToolOpKind, ToolId, ToolProperty, ToolTrait, ToolType,
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
        "Track substantial work with 1–16 stable steps, dependencies and acceptance criteria; skip simple tasks unless specific validation was requested. Record each requested Cargo check in a step's validation field using its exact Cargo tool parameters. Completion requires those checks to succeed on the current workspace. Batch progress into one update at meaningful milestones. Use current revisions from runtime context. Existing pending steps may complete directly with successful evidence source IDs and explanations; dependencies must be completed. New steps start pending or in_progress; at most one is in_progress. Reopen completed steps after requirements change and changed or blocked steps before completion. Does not change work mode."
    }
    fn field_properties() -> FnvHashMap<String, ToolProperty> {
        json!({
            "revision":{"type":"integer","description":"Current plan revision"},
            "requirements_revision":{"type":"integer","description":"Current requirements revision"},
            "steps":{"type":"array","minItems":1,"maxItems":16,"items":{"type":"object","additionalProperties":false,"properties":{
                "id":{"type":"string"},"description":{"type":"string"},"dependencies":{"type":"array","items":{"type":"string"}},
                "acceptance":{"type":"string"},"state":{"type":"string","enum":["pending","in_progress","completed","blocked"]},
                "evidence":{"type":"array","items":{"type":"object","additionalProperties":false,"properties":{"source":{"type":"string"},"explanation":{"type":"string"}},"required":["source","explanation"]}},
                "blocked_reason":{"type":["string","null"]},
                "validation":{"type":["object","null"],"description":"Exact Cargo tool input for a requested finite check (check, test, fmt_check, clippy or run); null for other work","properties":crate::tools::update_plan::validation_properties(),"required":["operation"]}
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
        for validation in input.update.steps.iter().filter_map(|step| step.validation.as_ref()) {
            serde_json::from_value::<tools::cargo_tools::CargoRequest>(serde_json::Value::Object(validation.cargo.clone()))?.validation_command()?;
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

fn validation_properties() -> serde_json::Map<String, Value> {
    tools::cargo_tools::Cargo::<tools::cargo_tools::AllOperations>::field_properties()
        .into_iter()
        .map(|(name, property)| (name, property.into_schema()))
        .collect()
}
