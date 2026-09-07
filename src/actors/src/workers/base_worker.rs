use crate::actor::{ActorContext, Dependency, IntoActorErr, Message};
use crate::actor_state::ActorState;
use crate::tools::gather_context::GatherContext;
use crate::tools::make_changes::MakeChanges;
use crate::worker::Worker;
use analysis::contexts::context::Context;
use analysis::contexts::rust_context::RustContext;
use async_trait::async_trait;
use ractor::{ActorProcessingErr, ActorRef};
use std::marker::PhantomData;
use tools::tool_defs::{ErasedToolRef, erased_tool};

pub struct BaseWorker<C: Context> {
    _ctx: PhantomData<C>,
}

const PROMPT: &str = include_str!("resources/base_worker.md");

#[async_trait]
impl Worker for BaseWorker<RustContext> {
    type C = RustContext;

    fn init_prompt(_: Option<&str>) -> String {
        PROMPT.to_owned()
    }

    async fn startup_hook(
        &self,
        myself: ActorRef<Message>,
        dependency: Dependency<Self::C>,
    ) -> Result<ActorState<Self::C>, ActorProcessingErr> {
        let file_actor_ref =
            crate::background_actors::file_actor::start(&dependency.context, &myself).await?;

        let state = ActorState::new(dependency, myself.clone(), Some(file_actor_ref))
            .await
            .actor_err()?;

        Ok(state)
    }

    fn tools() -> Vec<ErasedToolRef<Self::C, ActorContext<Self::C>>> {
        vec![
            erased_tool::<tools::find_files::FindFiles, Self::C, ActorContext<Self::C>>(),
            erased_tool::<tools::list_directory::ListDirectory, Self::C, ActorContext<Self::C>>(),
            erased_tool::<tools::inspect_context::InspectContext, Self::C, ActorContext<Self::C>>(),
            erased_tool::<GatherContext, Self::C, ActorContext<Self::C>>(),
            erased_tool::<MakeChanges, Self::C, ActorContext<Self::C>>(),
        ]
    }
}

impl<C: Context> BaseWorker<C> {
    pub fn new() -> Self {
        Self { _ctx: PhantomData }
    }
}
