use crate::actor::{ActorContext, Dependency, IntoActorErr, Message};
use crate::actor_state::ActorState;
use crate::background_actors::cache_actor::CacheActor;
use crate::background_actors::file_actor::FileActor;
use crate::background_actors::{cache_actor, file_actor};
use crate::tools::gather_context::GatherContext;
use crate::tools::make_changes::MakeChanges;
use crate::worker::Worker;
use analysis::contexts::context::Context;
use analysis::contexts::rust_context::RustContext;
use async_trait::async_trait;
use ractor::{Actor, ActorProcessingErr, ActorRef};
use std::marker::PhantomData;
use tools::tool_defs::{ErasedToolRef, erased_tool};

pub struct BaseWorker<C: Context> {
    _ctx: PhantomData<C>,
}

#[async_trait]
impl Worker for BaseWorker<RustContext> {
    type C = RustContext;

    fn init_prompt(_: Option<&str>) -> String {
        "You are a Rust coding orchestrator in a Rust codebase. You do not read or write files directly; you delegate focused work to specialized agents.

Use the project symbol context with call to `gather_context` with a narrow question that includes ALL relevant files, symbols, and assumptions.
When code changes are needed, call `make_changes` with the complete task, constraints, and the context the write worker needs.

Operate like a senior coding agent:
- Break broad requests into small, verifiable steps.
- Prefer existing patterns and local APIs over new abstractions.
- Keep unrelated user changes out of scope.
- Ask only when a risky assumption cannot be resolved through workers.
- Finish with a concise summary of the outcome and any validation reported by workers.

Keep worker instructions concrete and bounded."
            .to_owned()
    }

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
                proj: cur_context.rust_proj.clone(),
                cache_actor: cache_actor_ref,
            },
            myself.get_cell(),
        )
        .await?;

        let state = ActorState::new(dependency, myself.clone(), Some(file_actor_ref))
            .await
            .actor_err()?;

        Ok(state)
    }

    fn tools() -> Vec<ErasedToolRef<Self::C, ActorContext<Self::C>>> {
        vec![
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
