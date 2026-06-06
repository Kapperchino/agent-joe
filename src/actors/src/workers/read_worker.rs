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
        format!("You are read-only agent in a rust code base,\
     you will be asked a general question by the parent agent, make sure you are very sure of your answer and keep it concise\
     \n{question}").to_owned()
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
