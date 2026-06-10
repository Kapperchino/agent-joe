use crate::actor::{ActorContext, Dependency, IntoActorErr, Message};
use crate::actor_state::ActorState;
use crate::tools::validate_rust::ValidateRust;
use crate::worker::Worker;
use analysis::contexts::context::Context;
use analysis::contexts::rust_empty_context::RustEmptyContext;
use async_trait::async_trait;
use ractor::{ActorProcessingErr, ActorRef};
use std::marker::PhantomData;
use tools::apply_patch::ApplyPatch;
use tools::grep::GrepTool;
use tools::read_file::ReadFile;
use tools::tool_defs::{ErasedToolRef, erased_tool};

pub struct WriteWorker<C: Context> {
    _ctx: PhantomData<C>,
}

#[async_trait]
impl Worker for WriteWorker<RustEmptyContext> {
    type C = RustEmptyContext;

    fn init_prompt(added: Option<&str>) -> String {
        let question = added.unwrap_or_default();
        format!(
            "You are a write-enabled Rust coding agent. Your job is to make the requested code change in this workspace, using the surrounding code as the source of truth.

Operating principles:
- Inspect relevant files before editing; use `grep` to locate symbols and `read_file` for focused context.
- Prefer small, idiomatic Rust changes that match existing style and module boundaries.
- Preserve unrelated user changes and avoid broad rewrites.
- Use `apply_patch` for focused file edits.
- When the change should be checked, call `validate_rust` with enough context for an independent validation pass.
- Do not claim validation passed unless the validation agent actually reported success.
- Do not add tests unless explicitly asked

After the work is complete, respond to the orchestrator with the files changed, the behavioral effect, and the validation that was run or why it was not run.

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
            erased_tool::<ValidateRust, Self::C, ActorContext<Self::C>>(),
        ]
    }
}

impl<C: Context> WriteWorker<C> {
    pub fn new() -> Self {
        Self { _ctx: PhantomData }
    }
}
