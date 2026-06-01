use common_models::tui_models::{ActorToTui, ActorToTuiPacket, State};
use flume::Sender;
use ractor::ActorId;
#[derive(Clone)]
pub struct EventReporter {
    pub actor_id: ActorId,
    pub tui_tx: Sender<ActorToTui>,
}

impl EventReporter {
    pub fn state_changed(&self, new_state: State) {
        let _ = self.tui_tx.send(ActorToTui {
            actor_id: self.get_id(),
            packet: ActorToTuiPacket::StateChanged(new_state.clone()),
        });
    }

    pub fn send_delta(&self, str: String) {
        let _ = self.tui_tx.send(ActorToTui {
            actor_id: self.get_id(),
            packet: ActorToTuiPacket::Data(str),
        });
    }

    pub fn send(&self, item: ActorToTuiPacket) {
        let _ = self.tui_tx.send(ActorToTui {
            actor_id: self.get_id(),
            packet: item,
        });
    }

    fn get_id(&self) -> u64 {
        match self.actor_id {
            ActorId::Local(id) => id,
            ActorId::Remote { node_id, pid } => pid,
        }
    }
}
