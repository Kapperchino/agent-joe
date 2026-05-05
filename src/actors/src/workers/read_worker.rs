use crate::actor::{Dependency, IntoActorErr, Message};
use crate::actor_state::ActorState;
use crate::worker::Worker;
use analysis::contexts::context::Context;
use analysis::contexts::rust_context::RustContext;
use async_trait::async_trait;
use clients::tool_defs::{ReadFile, Tool};
use ractor::{ActorProcessingErr, ActorRef};
use std::marker::PhantomData;

pub struct ReadWorker<C: Context> {
    _ctx: PhantomData<C>,
}

#[async_trait]
impl Worker for ReadWorker<RustContext> {
    type C = RustContext;

    async fn startup_hook(
        &self,
        myself: ActorRef<Message>,
        dependency: Dependency<Self::C>,
    ) -> Result<ActorState<Self::C>, ActorProcessingErr> {
        let state = ActorState::new(dependency, None).await.actor_err()?;
        Ok(state)
    }

    fn tools() -> Vec<Tool> {
        vec![
            Tool::ReadFile(ReadFile::default()),
            Tool::Grep(clients::tool_defs::GrepTool::default()),
        ]
    }
}

impl<C: Context> ReadWorker<C> {
    pub fn new() -> Self {
        Self { _ctx: PhantomData }
    }
}
