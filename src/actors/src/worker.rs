use crate::actor::Dependency;
use crate::actor::Message;
use crate::actor_state::ActorState;
use analysis::contexts::context::Context;
use async_trait::async_trait;
use tools::tool_defs::ErasedToolRef;
use ractor::{ActorProcessingErr, ActorRef};

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
    type C: Context + Send + Sync + 'static;

    async fn startup_hook(
        &self,
        myself: ActorRef<Message>,
        dependency: Dependency<Self::C>,
    ) -> Result<ActorState<Self::C>, ActorProcessingErr>;

    fn tools() -> Vec<ErasedToolRef<Self::C>>;
}
