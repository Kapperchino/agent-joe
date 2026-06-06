use crate::actor::Message;
use crate::actor::{ActorContext, Dependency};
use crate::actor_state::ActorState;
use analysis::contexts::context::Context;
use async_trait::async_trait;
use ractor::{ActorProcessingErr, ActorRef};
use tools::tool_defs::ErasedToolRef;

pub struct WorkerAdapter<W> {
    pub(crate) worker: W,
}

impl<W> WorkerAdapter<W> {
    pub fn new(worker: W) -> Self {
        Self { worker }
    }

    pub fn into_inner(self) -> W {
        self.worker
    }
}

#[async_trait]
pub trait Worker: Send + Sync + 'static {
    type C: Context + Send + Sync + Clone + 'static;

    fn init_prompt(added: Option<&str>) -> String;

    async fn startup_hook(
        &self,
        myself: ActorRef<Message>,
        dependency: Dependency<Self::C>,
    ) -> Result<ActorState<Self::C>, ActorProcessingErr>;

    fn tools() -> Vec<ErasedToolRef<Self::C, ActorContext<Self::C>>>;
}
