use crate::actor::{ActorContext, Dependency, IntoActorErr, Message};
use crate::actor_state::ActorState;
use crate::worker::Worker;
use analysis::contexts::context::Context;
use analysis::contexts::rust_context::RustContext;
use async_trait::async_trait;
use ractor::{ActorProcessingErr, ActorRef};
use std::marker::PhantomData;
use tools::apply_patch::ApplyPatch;
use tools::cargo_check::CargoCheck;
use tools::cargo_test::CargoTest;
use tools::grep::GrepTool;
use tools::read_file::ReadFile;
use tools::tool_defs::{ErasedToolRef, erased_tool};
use tools::web_search::WebSearch;

pub struct SimpleWorker<C: Context> {
    _ctx: PhantomData<C>,
}

#[async_trait]
impl Worker for SimpleWorker<RustContext> {
    type C = RustContext;

    fn init_prompt(added: Option<&str>) -> String {
        let question = added.unwrap_or_default();
        format!(
            "You are a pragmatic Rust coding agent in a Rust codebase with direct read, write, cargo validation, and web search tools.

Operate end-to-end:
- Understand the request and inspect relevant context before changing code.
- Prefer existing patterns, local helper APIs, and small focused patches.
- Preserve unrelated user changes; do not rewrite or revert code outside the task.
- Ask only when a risky assumption cannot be resolved from the workspace.
- Do not claim validation succeeded unless you actually ran the validation tool.

Use the tools deliberately:
- `grep`: search project files when you need to discover symbols, call sites, or related code.
- `read_file`: read known files or focused line ranges before making or explaining code changes.
- `apply_patch`: make small, focused edits that preserve the surrounding style.
- `cargo_check`: check whether the project compiles, including warnings only when they are relevant.
- `cargo_test`: run targeted tests first, then broader tests when the change warrants it.
- `web_search`: look up current external information only when local project context is insufficient.

When finished, respond concisely with what changed and what validation was run.

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
            erased_tool::<ApplyPatch, Self::C, ActorContext<Self::C>>(),
            erased_tool::<CargoCheck, Self::C, ActorContext<Self::C>>(),
            erased_tool::<CargoTest, Self::C, ActorContext<Self::C>>(),
            erased_tool::<WebSearch, Self::C, ActorContext<Self::C>>(),
        ]
    }
}

impl<C: Context> SimpleWorker<C> {
    pub fn new() -> Self {
        Self { _ctx: PhantomData }
    }
}
