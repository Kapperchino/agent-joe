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

pub async fn run_worker<W: Worker>(
    worker: W,
    dependency: Dependency<W::C>,
    parent: ActorRef<Message>,
) -> Result<String, WorkerFailure> {
    use ractor::Actor;
    let owner = utils::execution::ExecutionScope::current();
    let scope = dependency.runtime.scope.clone();
    let cancel_on_drop = scope.cancel.clone().drop_guard();
    let registration = owner.register(
        utils::execution::ResourceKind::Worker,
        "Delegated worker".into(),
    );
    let handle = owner.tasks.spawn(async move {
        let _registration = registration;
        let spawned = Actor::spawn_linked(
            None,
            WorkerAdapter::new(worker),
            dependency,
            parent.get_cell(),
        )
        .await;
        let result = match spawned {
            Ok((actor, handle)) => RunningWorker { actor, handle }.run(&scope).await,
            Err(error) => Err(WorkerFailure::Startup(error.to_string())),
        };
        scope.finish().await;
        result
    });
    let result = handle
        .await
        .map_err(|error| WorkerFailure::Join(error.to_string()))
        .and_then(std::convert::identity);
    drop(cancel_on_drop);
    result
}

struct RunningWorker {
    actor: ActorRef<Message>,
    handle: tokio::task::JoinHandle<()>,
}
impl RunningWorker {
    async fn run(
        mut self,
        scope: &utils::execution::ExecutionScope,
    ) -> Result<String, WorkerFailure> {
        let (tx, mut rx) = tokio::sync::oneshot::channel();
        match self.actor.send_message(Message::RunWorker(tx.into())) {
            Ok(()) => tokio::select! {
                biased;
                reply = &mut rx => {
                    let result = reply.unwrap_or(Err(WorkerFailure::Stopped));
                    self.stop().await;
                    result
                }
                _ = scope.cancel.cancelled() => {
                    let _ = self.actor.send_message(Message::Interrupt);
                    let _ = rx.await;
                    self.stop().await;
                    Err(WorkerFailure::Cancelled)
                }
                result = &mut self.handle => Err(match result {
                    Ok(()) => WorkerFailure::Stopped,
                    Err(error) => WorkerFailure::Join(error.to_string()),
                }),
            },
            Err(error) => {
                self.stop().await;
                Err(WorkerFailure::Mailbox(error.to_string()))
            }
        }
    }

    async fn stop(self) {
        let _ = self.actor.send_message(Message::Interrupt);
        self.actor.stop(None);
        let _ = self.handle.await;
    }
}

#[derive(Debug, thiserror::Error)]
pub enum WorkerFailure {
    #[error("Worker is already running")]
    AlreadyRunning,
    #[error("Worker startup failed: {0}")]
    Startup(String),
    #[error("Worker could not start its turn: {0}")]
    Mailbox(String),
    #[error("Worker failed: {0}")]
    Turn(clients::failure::Failure),
    #[error("Worker cancelled")]
    Cancelled,
    #[error("Worker stopped without a result")]
    Stopped,
    #[error("Worker task terminated: {0}")]
    Join(String),
}
impl WorkerFailure {
    pub fn into_tool_failure(self) -> tools::tool_error::ToolFailure {
        use tools::tool_error::{ToolEffects, ToolFailure, ToolFailureKind};
        let effects = match self {
            Self::Startup(_) | Self::AlreadyRunning => ToolEffects::NotStarted,
            _ => ToolEffects::MayHaveChanged,
        };
        ToolFailure::new(ToolFailureKind::Worker, effects, self.to_string())
    }
}
