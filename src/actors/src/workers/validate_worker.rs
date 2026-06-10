use crate::actor::{ActorContext, Dependency, IntoActorErr, Message};
use crate::actor_state::ActorState;
use crate::worker::Worker;
use analysis::contexts::context::Context;
use analysis::contexts::rust_empty_context::RustEmptyContext;
use async_trait::async_trait;
use ractor::{ActorProcessingErr, ActorRef};
use std::marker::PhantomData;
use tools::cargo_check::CargoCheck;
use tools::cargo_test::CargoTest;
use tools::tool_defs::{ErasedToolRef, erased_tool};

pub struct ValidateWorker<C: Context> {
    _ctx: PhantomData<C>,
}

#[async_trait]
impl Worker for ValidateWorker<RustEmptyContext> {
    type C = RustEmptyContext;

    fn init_prompt(added: Option<&str>) -> String {
        let question = added.unwrap_or_default();
        format!(
            "You are a Rust validation agent. Your only responsibility is to determine whether the workspace is good to go for the supplied context.

Use validation tools, not speculation:
- Start with `cargo_check` for compilation errors; include warnings when they are relevant to the request or failure.
- Run `cargo_test` when tests are requested, affected behavior has test coverage, or compilation alone is not enough.
- Prefer targeted tests by package or test name when the context identifies them; otherwise run the broader test command that best fits the risk.

Do not edit files. Report the exact validation commands/tools used, whether they passed or failed, and the most relevant errors or failing tests. If you cannot validate something with the available tools, say so directly.

{question}"
        )
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
        vec![
            erased_tool::<CargoCheck, Self::C, ActorContext<Self::C>>(),
            erased_tool::<CargoTest, Self::C, ActorContext<Self::C>>(),
        ]
    }
}

impl<C: Context> ValidateWorker<C> {
    pub fn new() -> Self {
        Self { _ctx: PhantomData }
    }
}
