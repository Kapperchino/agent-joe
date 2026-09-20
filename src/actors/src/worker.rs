use crate::actor::Message;
use crate::actor::{ActorContext, Dependency};
use crate::states::actor_state::ActorState;
use crate::states::runtime::ExecutionRole;
use analysis::contexts::context::Context;
use async_trait::async_trait;
use ractor::{Actor, ActorProcessingErr, ActorRef, SupervisionEvent};
use tools::tool_defs::ErasedToolRef;

pub struct WorkerAdapter<W> {
    pub worker: W,
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
    type Msg: ractor::Message;
    type State: Send + 'static;
    type Arguments: Send + 'static;

    async fn start(
        &self,
        myself: ActorRef<Self::Msg>,
        arguments: Self::Arguments,
    ) -> Result<Self::State, ActorProcessingErr>;

    async fn handle(
        &self,
        myself: ActorRef<Self::Msg>,
        message: Self::Msg,
        state: &mut Self::State,
    ) -> Result<(), ActorProcessingErr>;

    async fn stop(
        &self,
        _: ActorRef<Self::Msg>,
        _: &mut Self::State,
    ) -> Result<(), ActorProcessingErr> {
        Ok(())
    }
}

#[cfg_attr(feature = "async-trait", ractor::async_trait)]
impl<W: Worker> Actor for WorkerAdapter<W> {
    type Msg = W::Msg;
    type State = W::State;
    type Arguments = W::Arguments;

    async fn pre_start(
        &self,
        myself: ActorRef<Self::Msg>,
        arguments: Self::Arguments,
    ) -> Result<Self::State, ActorProcessingErr> {
        self.worker.start(myself, arguments).await
    }

    async fn post_stop(
        &self,
        myself: ActorRef<Self::Msg>,
        state: &mut Self::State,
    ) -> Result<(), ActorProcessingErr> {
        self.worker.stop(myself, state).await
    }

    async fn handle(
        &self,
        myself: ActorRef<Self::Msg>,
        message: Self::Msg,
        state: &mut Self::State,
    ) -> Result<(), ActorProcessingErr> {
        self.worker.handle(myself, message, state).await
    }

    async fn handle_supervisor_evt(
        &self,
        _: ActorRef<Self::Msg>,
        event: SupervisionEvent,
        _: &mut Self::State,
    ) -> Result<(), ActorProcessingErr> {
        if let SupervisionEvent::ActorFailed(who, reason) = event {
            tracing::error!("Child actor {:?} failed: {:?}", who.get_id(), reason);
        }
        Ok(())
    }
}

#[async_trait]
pub trait ContextWorker: Send + Sync + 'static {
    type C: Context + Send + Sync + Clone + 'static;

    fn init_prompt(added: Option<&str>) -> String;

    async fn startup_hook(
        &self,
        myself: ActorRef<Message>,
        dependency: Dependency<Self::C>,
    ) -> Result<ActorState<Self::C>, ActorProcessingErr>;

    fn tools() -> Vec<ErasedToolRef<Self::C, ActorContext<Self::C>>>;
}

pub async fn run_worker<W: ContextWorker>(
    worker: W,
    mut dependency: Dependency<W::C>,
    parent: ActorRef<Message>,
) -> Result<String, WorkerFailure> {
    dependency.runtime.role = match dependency.runtime.role {
        ExecutionRole::Root => ExecutionRole::Helper,
        role => role,
    };
    let owner = utils::execution::ExecutionScope::current();
    let scope = dependency.runtime.scope.clone();
    let cancel_on_drop = scope.cancel.clone().drop_guard();
    let registration = owner.register(
        utils::execution::ResourceKind::Worker,
        "Delegated worker".into(),
    );
    let (reply, receive) = tokio::sync::oneshot::channel();
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
            Ok((actor, handle)) => {
                RunningWorker { actor, handle }
                    .run(&scope, reply, receive)
                    .await
            }
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
        tx: tokio::sync::oneshot::Sender<Result<String, WorkerFailure>>,
        mut rx: tokio::sync::oneshot::Receiver<Result<String, WorkerFailure>>,
    ) -> Result<String, WorkerFailure> {
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

pub use turn_engine::WorkerFailure;

#[derive(Default)]
pub struct WorkerReplies {
    pending: std::collections::HashMap<
        common_models::runtime_ids::OperationId,
        ractor::RpcReplyPort<Result<String, WorkerFailure>>,
    >,
}

impl WorkerReplies {
    pub fn insert(
        &mut self,
        reply: ractor::RpcReplyPort<Result<String, WorkerFailure>>,
    ) -> common_models::runtime_ids::OperationId {
        let request = common_models::runtime_ids::OperationId::new();
        self.pending.insert(request, reply);
        request
    }

    pub fn complete(
        &mut self,
        request: common_models::runtime_ids::OperationId,
        result: Result<String, WorkerFailure>,
    ) {
        if let Some(reply) = self.pending.remove(&request) {
            let _ = reply.send(result);
        }
    }
}
