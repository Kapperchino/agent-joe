use crate::actor::{Dependency, IntoActorErr, Message};
use crate::actor_state::ActorState;
use crate::background_actors::cache_actor::CacheActor;
use crate::background_actors::file_actor::FileActor;
use crate::background_actors::{cache_actor, file_actor};
use crate::worker::Worker;
use analysis::rust_context::{Context, RustContext};
use async_trait::async_trait;
use ractor::{Actor, ActorProcessingErr, ActorRef};
use std::marker::PhantomData;

pub struct BaseWorker<C: Context> {
    _ctx: PhantomData<C>,
}

#[async_trait]
impl Worker for BaseWorker<RustContext> {
    type C = RustContext;

    async fn startup_hook(
        &self,
        myself: ActorRef<Message>,
        dependency: Dependency<Self::C>,
    ) -> Result<ActorState<Self::C>, ActorProcessingErr> {
        let cur_context = &dependency.context;

        let (cache_actor_ref, _) = Actor::spawn_linked(
            None,
            CacheActor {},
            cache_actor::Dependency {
                symbol_cache: cur_context.symbol_cache.clone(),
                proj: cur_context.rust_proj.clone(),
            },
            myself.get_cell(),
        )
        .await?;

        let (file_actor_ref, _) = Actor::spawn_linked(
            None,
            FileActor {},
            file_actor::Dependency {
                main_dir: cur_context.cur_dir.clone(),
                vfs: cur_context.rust_proj.vfs.clone(),
                a_host: cur_context.rust_proj.analysis_host.clone(),
                cache_actor: cache_actor_ref,
            },
            myself.get_cell(),
        )
        .await?;

        let state = ActorState::new(dependency, file_actor_ref)
            .await
            .actor_err()?;

        Ok(state)
    }
}

impl<C: Context> BaseWorker<C> {
    pub fn new() -> Self {
        Self { _ctx: PhantomData }
    }
}
