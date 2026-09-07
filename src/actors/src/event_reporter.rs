use common_models::tui_models::{ActorToTui, ActorToTuiPacket, State, TokenCount};
use flume::Sender;
#[derive(Clone)]
pub enum EventReporter {
    Interactive {
        actor_id: u64,
        tui_tx: Sender<ActorToTui>,
    },
    Compaction(crate::provider_task::ProviderTarget),
}

impl EventReporter {
    pub fn state_changed(&self, new_state: State) {
        self.send(ActorToTuiPacket::StateChanged(new_state));
    }

    pub fn send_delta(&self, text: String) {
        self.send(ActorToTuiPacket::Data(text));
    }

    pub fn send(&self, item: ActorToTuiPacket) {
        if let Self::Interactive { actor_id, tui_tx } = self {
            let _ = tui_tx.send(ActorToTui {
                actor_id: *actor_id,
                packet: item,
            });
        }
    }

    pub fn usage(&self, usage: TokenCount) {
        if let Self::Compaction(target) = self {
            let _ = target.send(crate::provider_task::ProviderEvent::CompactionUsage(usage));
        }
    }
}
