use ractor::{Actor, ActorProcessingErr, ActorRef, SupervisionEvent};
use tracing::error;

pub struct WorkerSupervisor;

#[cfg_attr(feature = "async-trait", ractor::async_trait)]
impl Actor for WorkerSupervisor {
    type Msg = ();
    type State = ();
    type Arguments = ();

    async fn pre_start(
        &self,
        _myself: ActorRef<Self::Msg>,
        _args: (),
    ) -> Result<Self::State, ActorProcessingErr> {
        Ok(())
    }

    async fn handle(
        &self,
        _myself: ActorRef<Self::Msg>,
        _message: Self::Msg,
        _state: &mut Self::State,
    ) -> Result<(), ActorProcessingErr> {
        Ok(())
    }

    async fn handle_supervisor_evt(
        &self,
        _myself: ActorRef<Self::Msg>,
        message: SupervisionEvent,
        _state: &mut Self::State,
    ) -> Result<(), ActorProcessingErr> {
        match message {
            SupervisionEvent::ActorFailed(who, reason) => {
                error!("Worker {:?} failed: {:?}", who.get_id(), reason);
            }
            SupervisionEvent::ActorTerminated(who, _, reason) => {
                reason.inspect(|reason| {
                    error!("Worker {:?} terminated: {:?}", who.get_id(), reason);
                });
            }
            _ => {}
        }
        Ok(())
    }
}
