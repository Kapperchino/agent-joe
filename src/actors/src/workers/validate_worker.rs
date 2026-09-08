use crate::actor::{ActorContext, Dependency, IntoActorErr, Message};
use crate::actor_state::ActorState;
use crate::worker::Worker;
use analysis::contexts::context::Context;
use analysis::contexts::rust_empty_context::RustEmptyContext;
use async_trait::async_trait;
use ractor::{ActorProcessingErr, ActorRef};
use std::marker::PhantomData;
use tools::tool_defs::{ErasedToolRef, erased_tool};

pub struct ValidateWorker<C: Context> {
    _ctx: PhantomData<C>,
}

const PROMPT: &str = include_str!("resources/validate_worker.md");

#[async_trait]
impl Worker for ValidateWorker<RustEmptyContext> {
    type C = RustEmptyContext;

    fn init_prompt(added: Option<&str>) -> String {
        let question = added.unwrap_or_default();
        format!("{PROMPT}\n\n{question}")
    }

    async fn startup_hook(
        &self,
        myself: ActorRef<Message>,
        dependency: Dependency<Self::C>,
    ) -> Result<ActorState<Self::C>, ActorProcessingErr> {
        let state = ActorState::new(dependency, myself.clone(), None)
            .await
            .actor_err()?;
        Ok(state)
    }

    fn tools() -> Vec<ErasedToolRef<Self::C, ActorContext<Self::C>>> {
        vec![erased_tool::<
            tools::cargo_tools::Cargo<tools::cargo_tools::ValidationOperations>,
            Self::C,
            ActorContext<Self::C>,
        >()]
    }
}

impl<C: Context> ValidateWorker<C> {
    pub fn new() -> Self {
        Self { _ctx: PhantomData }
    }
}
