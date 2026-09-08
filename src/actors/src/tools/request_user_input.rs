use crate::actor::{ActorContext, Message};
use analysis::contexts::context::Context;
use async_trait::async_trait;
use common_models::interaction::Question;
use serde_json::{Value, json};
use tools::tool_defs::{
    LenientDeserialize, ToolDefTrait, ToolEffect, ToolId, ToolProperty, ToolTrait, ToolType,
};
use utils::utils::FnvHashMap;

pub struct RequestUserInput;

#[derive(Clone, serde::Deserialize)]
#[serde(transparent)]
pub struct Input(Question);

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

impl ToolDefTrait for RequestUserInput {
    fn tool_name() -> &'static str {
        "request_user_input"
    }
    fn tool_description() -> &'static str {
        "Ask a structured clarification question with an unused ID, choices and/or free text. Required questions pause continuation after current work is cleaned up; optional questions allow independent work. Only the user can answer. Answers cannot widen workspace permissions."
    }
    fn field_properties() -> FnvHashMap<String, ToolProperty> {
        json!({
            "id": {"type":"string","description":"Unused literal question ID, up to 64 ASCII letters, digits, underscores or hyphens"},
            "prompt": {"type":"string","description":"Question for the user, up to 2048 bytes"},
            "required": {"type":"boolean","description":"True pauses work until explicitly answered"},
            "choices": {"type":"array","maxItems":6,"items":{"type":"object","additionalProperties":false,"properties":{"id":{"type":"string"},"label":{"type":"string"}},"required":["id","label"]}},
            "allow_free_text": {"type":"boolean","description":"Allow a typed text answer; default true"}
}).as_object().unwrap().iter()
            .map(|(name, schema)| (name.clone(), ToolProperty::Schema(schema.clone()))).collect()
    }
    fn required_fields() -> Vec<String> {
        ["id", "prompt", "required"]
            .into_iter()
            .map(str::to_owned)
            .collect()
    }
    fn req(&self) -> anyhow::Result<FnvHashMap<String, String>> {
        Ok(Default::default())
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
            ActorContext::ActorInfo(info) if info.dep.runtime.worker.is_none() => Ok(info),
            _ => Err(anyhow::anyhow!("User interaction requires the root actor")),
        }?;
        let (reply, receive) = tokio::sync::oneshot::channel();
        info.actor_ref
            .send_message(Message::AskQuestion {
                question: input.0,
                scope: crate::actor::InteractionScope(info.dep.runtime.scope.clone()),
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
    fn effect() -> ToolEffect {
        ToolEffect::Interaction
    }
    fn tool_type() -> ToolType {
        ToolType::Client
    }
}
