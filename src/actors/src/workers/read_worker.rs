use crate::actor::{ActorContext, Dependency, IntoActorErr, Message};
use crate::actor_state::ActorState;
use crate::worker::Worker;
use analysis::contexts::context::Context;
use analysis::contexts::rust_empty_context::RustEmptyContext;
use async_trait::async_trait;
use ractor::{ActorProcessingErr, ActorRef};
use std::marker::PhantomData;
use tools::grep::GrepTool;
use tools::read_file::ReadFile;
use tools::tool_defs::{ErasedToolRef, erased_tool};
use tools::web_search::WebSearch;

pub struct ReadWorker<C: Context> {
    _ctx: PhantomData<C>,
}

#[async_trait]
impl Worker for ReadWorker<RustEmptyContext> {
    type C = RustEmptyContext;

    fn init_prompt(added: Option<&str>) -> String {
        let question = added.unwrap_or_default();
        format!(
            "You are a read-only Rust code investigator. You can inspect files and search the project, but you must not propose or perform edits unless the parent agent explicitly asked for an implementation plan.

Use the tools deliberately:
- `grep`: find symbols, call sites, tests, and related modules before answering.
- `read_file`: inspect exact code around relevant matches before drawing conclusions.
- `web_search`: use only when current external facts or docs are required and local context is insufficient.

Answer from evidence. Prefer file paths, symbols, and concrete behavior over speculation. If the answer is uncertain, say what is uncertain and what additional context would resolve it. Keep the response concise and directly useful to the parent agent.

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
            erased_tool::<ReadFile, Self::C, ActorContext<Self::C>>(),
            erased_tool::<GrepTool, Self::C, ActorContext<Self::C>>(),
            erased_tool::<WebSearch, Self::C, ActorContext<Self::C>>(),
        ]
    }
}

impl<C: Context> ReadWorker<C> {
    pub fn new() -> Self {
        Self { _ctx: PhantomData }
    }
}
