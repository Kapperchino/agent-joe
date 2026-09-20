use common_models::tui_models::{ActorToTui, ActorToTuiPacket, State, TokenCount};
use flume::Sender;
#[derive(Clone)]
pub enum EventReporter {
    Interactive {
        actor_id: u64,
        tui_tx: Sender<ActorToTui>,
    },
    Compaction(crate::states::provider_task::ProviderTarget),
}

impl EventReporter {
    pub fn validation(&self, result: &tools::tool_defs::ToolResult) {
        use common_models::tui_models::{ValidationProgress, ValidationState};
        let operation = result
            .invocation
            .input
            .get("operation")
            .and_then(serde_json::Value::as_str);
        if let ("cargo", Some(operation @ ("check" | "test" | "clippy" | "fmt_check"))) =
            (result.invocation.name.as_ref(), operation)
        {
            let state = match &result.outcome {
                Ok(_) => ValidationState::Passed,
                Err(failure) if failure.effects == tools::tool_error::ToolEffects::NotStarted => {
                    ValidationState::NotRun
                }
                Err(_) => ValidationState::Failed,
            };
            self.send(ActorToTuiPacket::ValidationUpdated(ValidationProgress {
                operation: operation.into(),
                state,
            }));
        }
    }

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
            let _ =
                target.send(crate::states::provider_task::ProviderEvent::CompactionUsage(usage));
        }
    }
}

impl common_models::tui_models::EventSink for EventReporter {
    fn send(&self, packet: ActorToTuiPacket) {
        EventReporter::send(self, packet);
    }
}
