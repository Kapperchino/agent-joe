use common_models::tui_models::{ActorToTui, ActorToTuiPacket, State};
use flume::Sender;
#[derive(Clone)]
pub struct EventReporter {
    pub actor_id: u64,
    pub tui_tx: Sender<ActorToTui>,
}

impl EventReporter {
    pub fn state_changed(&self, new_state: State) {
        self.send(ActorToTuiPacket::StateChanged(new_state));
    }

    pub fn send_delta(&self, text: String) {
        self.send(ActorToTuiPacket::Data(text));
    }

    pub fn send(&self, item: ActorToTuiPacket) {
        let _ = self.tui_tx.send(ActorToTui {
            actor_id: self.actor_id,
            packet: item,
        });
    }
}
