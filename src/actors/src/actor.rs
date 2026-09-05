use crate::{
    actor_state::ActorState,
    provider_task::ProviderEvent,
    scheduler::ToolEvent,
    turn::{FollowUp, HistoryDisposition, Tag},
    turn_machine::{Event, SessionEvent},
    worker::{Worker, WorkerAdapter, WorkerFailure},
};
use analysis::contexts::context::Context;
use clients::llm::LLmClient;
use commands::command::Command;
use common_models::{runtime_ids::TurnId, tui_models::ActorToTui};
use flume::Sender;
use ractor::{Actor, ActorProcessingErr, ActorRef, RpcReplyPort, SupervisionEvent};
use tools::tool_defs::ErasedToolRef;

pub trait IntoActorErr<T> {
    fn actor_err(self) -> Result<T, ActorProcessingErr>;
}
impl<T, E: std::fmt::Display> IntoActorErr<T> for Result<T, E> {
    fn actor_err(self) -> Result<T, ActorProcessingErr> {
        self.map_err(|e| ActorProcessingErr::from(e.to_string()))
    }
}

#[derive(Debug)]
pub enum Message {
    StartWork(Option<String>),
    #[cfg(test)]
    Inspect(RpcReplyPort<Vec<clients::llm::Message>>),
    RunWorker(RpcReplyPort<Result<String, WorkerFailure>>),
    Command(Command),
    Provider {
        tag: Tag,
        event: ProviderEvent,
    },
    Tools {
        tag: Tag,
        event: ToolEvent,
    },
    CleanupFinished {
        turn: TurnId,
    },
    Interrupt,
    Clear,
    KYS,
}

#[derive(Clone)]
pub struct Dependency<C: Context> {
    pub client: LLmClient,
    pub tools: Vec<ErasedToolRef<C, ActorContext<C>>>,
    pub tui_tx: Sender<ActorToTui>,
    pub debug_mode: bool,
    pub context: C,
    pub runtime: crate::runtime::Runtime,
}
impl<C: Context> Dependency<C> {
    pub fn tool(&self, name: &str) -> Option<&ErasedToolRef<C, ActorContext<C>>> {
        self.tools.iter().find(|tool| tool.name() == name)
    }
}

pub enum ActorContext<C: Context> {
    Noop,
    ActorInfo(ActorInfo<C>),
}
pub struct ActorInfo<C: Context> {
    pub dep: Dependency<C>,
    pub actor_ref: ActorRef<Message>,
}
#[cfg_attr(feature = "async-trait", ractor::async_trait)]
impl<W: Worker> Actor for WorkerAdapter<W> {
    type Msg = Message;
    type State = ActorState<W::C>;
    type Arguments = Dependency<W::C>;

    async fn pre_start(
        &self,
        myself: ActorRef<Message>,
        dependency: Self::Arguments,
    ) -> Result<Self::State, ActorProcessingErr> {
        let scope = dependency.runtime.scope.clone();
        tokio::select! {
            biased;
            _ = scope.cancel.cancelled() => Err("Worker startup cancelled".into()),
            state = scope.enter(self.worker.startup_hook(myself, dependency)) => state,
        }
    }

    async fn handle(
        &self,
        _: ActorRef<Message>,
        message: Message,
        state: &mut Self::State,
    ) -> Result<(), ActorProcessingErr> {
        let scope = state.dependency.runtime.scope.clone();
        scope
            .enter(async {
                match message {
                    Message::StartWork(prompt) => {
                        state
                            .dispatch(SessionEvent::Start(FollowUp::new(prompt)))
                            .await
                    }
                    Message::RunWorker(reply) => {
                        state.dispatch(SessionEvent::StartWorker(reply)).await
                    }
                    Message::Provider { tag, event } => state.provider_event(tag, event).await,
                    Message::Tools { tag, event } => {
                        let revision = state.dependency.runtime.workspace.revision();
                        state
                            .dispatch(SessionEvent::Tools {
                                tag,
                                event,
                                revision,
                            })
                            .await;
                    }
                    Message::CleanupFinished { turn } => {
                        state.dispatch(SessionEvent::CleanupFinished(turn)).await
                    }
                    Message::Interrupt => {
                        state
                            .dispatch(SessionEvent::Interrupt(HistoryDisposition::Retain))
                            .await
                    }
                    Message::Clear => {
                        state
                            .dispatch(SessionEvent::Interrupt(HistoryDisposition::Clear))
                            .await
                    }
                    Message::Command(command) => state.command(command).await,
                    Message::KYS => state.actor_ref.stop(None),
                    #[cfg(test)]
                    Message::Inspect(reply) => {
                        let _ = reply.send(state.visible_history());
                    }
                }
            })
            .await;
        Ok(())
    }

    async fn post_stop(
        &self,
        _: ActorRef<Message>,
        state: &mut Self::State,
    ) -> Result<(), ActorProcessingErr> {
        state.dispatch(Event::Shutdown).await;
        Ok(())
    }

    async fn handle_supervisor_evt(
        &self,
        _: ActorRef<Message>,
        event: SupervisionEvent,
        _: &mut Self::State,
    ) -> Result<(), ActorProcessingErr> {
        if let SupervisionEvent::ActorFailed(who, reason) = event {
            tracing::error!("Child actor {:?} failed: {:?}", who.get_id(), reason);
        }
        Ok(())
    }
}
