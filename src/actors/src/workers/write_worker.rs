use crate::actor::{ActorContext, Dependency, IntoActorErr, Message};
use crate::actor_state::ActorState;
use crate::tools::validate_rust::ValidateRust;
use crate::worker::Worker;
use analysis::contexts::context::Context;
use analysis::contexts::rust_empty_context::RustEmptyContext;
use async_trait::async_trait;
use ractor::{ActorProcessingErr, ActorRef};
use tools::apply_patch::ApplyPatch;
use std::marker::PhantomData;
use tools::grep::GrepTool;
use tools::insert_after_line::InsertAfterLine;
use tools::read_file::ReadFile;
use tools::string_replace::StringReplace;
use tools::tool_defs::{erased_tool, ErasedToolRef};

pub struct WriteWorker<C: Context> {
    _ctx: PhantomData<C>,
}

#[async_trait]
impl Worker for WriteWorker<RustEmptyContext> {
    type C = RustEmptyContext;

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
            erased_tool::<InsertAfterLine, Self::C, ActorContext<Self::C>>(),
            erased_tool::<StringReplace, Self::C, ActorContext<Self::C>>(),
            erased_tool::<ValidateRust, Self::C, ActorContext<Self::C>>(),
        ]
    }
}

impl<C: Context> WriteWorker<C> {
    pub fn new() -> Self {
        Self { _ctx: PhantomData }
    }
}
