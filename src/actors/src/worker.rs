use crate::actor::Message;
use crate::actor::{Dependency, IntoActorErr};
use crate::actor_state::ActorState;
use crate::background_actors::cache_actor::CacheActor;
use crate::background_actors::file_actor::FileActor;
use crate::background_actors::file_actor;
use analysis::rust_context::{Context, RustContext};
use async_trait::async_trait;
use ractor::{Actor, ActorCell, ActorProcessingErr, ActorRef};
use std::marker::PhantomData;
use crate::background_actors::cache_actor;

#[async_trait]
pub trait Worker:
    Actor<Msg = Message, State = ActorState<Self::C>, Arguments = Dependency<Self::C>>
{
    type C: Context;

    async fn startup_hook(
        &self,
        myself: ActorRef<Self::Msg>,
        dependency: Self::Arguments,
    ) -> Result<Self::State, ActorProcessingErr>;
}
