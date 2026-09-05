use crate::turn::Tag;
use common_models::{
    runtime_ids::TurnId,
    tui_models::{ActorToTui, ActorToTuiPacket, Lifecycle, State},
};
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
    pub fn turn(&self, id: TurnId, state: Lifecycle, detail: Option<String>) {
        self.send(ActorToTuiPacket::TurnChanged {
            turn_id: id,
            state,
            detail,
        });
    }
    pub fn operation(&self, tag: Tag, state: Lifecycle, detail: impl Into<String>) {
        self.send(ActorToTuiPacket::OperationChanged {
            turn_id: tag.turn,
            operation_id: tag.operation,
            state,
            detail: detail.into(),
        });
    }
}
