use flume::Sender;
use common_models::tui_models::{ActorToTui, State};
use tokio::sync::mpsc;
#[derive(Clone)]
pub struct EventReporter {
    pub tui_tx: Sender<ActorToTui>,
}

impl EventReporter {
    pub fn state_changed(&self, new_state: State) {
        let _ = self
            .tui_tx
            .send(ActorToTui::StateChanged(new_state.clone()));
    }

    pub fn send_delta(&self, str: String) {
        let _ = self.tui_tx.send(ActorToTui::Data(str));
    }

    pub fn send(&self, item: ActorToTui) {
        let _ = self.tui_tx.send(item);
    }
}
