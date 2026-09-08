use crate::{
    actor::{ActorContext, Dependency, IntoActorErr, Message},
    actor_state::ActorState,
    worker::Worker,
};
use analysis::contexts::rust_context::RustContext;
use async_trait::async_trait;
use ractor::{ActorProcessingErr, ActorRef};
use tools::tool_defs::ErasedToolRef;

pub struct TaskWorker;

#[async_trait]
impl Worker for TaskWorker {
    type C = RustContext;

    fn init_prompt(_: Option<&str>) -> String {
        "Complete the bounded worker request using only its allowed tools and paths. Preserve inherited parent/user constraints and scoped repository instructions. Work directly; delegation depth is one. Add focused regression coverage when behavior warrants it. Run targeted validation before broader checks when allowed. Explain findings, completed work, remaining issues, requested versus executed checks, and limitations concisely. The runtime attaches observed changes, tool validation evidence, usage and artifact references to your report. A successful compilation alone does not establish behavioral correctness.".into()
    }

    async fn startup_hook(
        &self,
        actor: ActorRef<Message>,
        dependency: Dependency<Self::C>,
    ) -> Result<ActorState<Self::C>, ActorProcessingErr> {
        ActorState::new(dependency, actor, None).await.actor_err()
    }

    fn tools() -> Vec<ErasedToolRef<Self::C, ActorContext<Self::C>>> {
        crate::workers::simple_worker::SimpleWorker::<RustContext>::tools()
    }
}
