use crate::runtime_ids::{OperationId, TurnId};
use commands::command::Command;

pub struct ActorToTui {
    pub actor_id: u64,
    pub packet: ActorToTuiPacket,
}
#[derive(Debug, Clone)]
pub enum ActorToTuiPacket {
    StateChanged(State),
    TurnChanged {
        turn_id: TurnId,
        state: Lifecycle,
        detail: Option<String>,
    },
    OperationChanged {
        turn_id: TurnId,
        operation_id: OperationId,
        state: Lifecycle,
        detail: String,
    },
    Queued {
        turn_id: TurnId,
        position: usize,
    },
    Data(String),
    ToolUse(Vec<String>),
    CommandResult(Command, String),
    TokensUpdated(TokenCount),
}

#[derive(Debug, Clone, Default)]
pub struct TokenCount {
    pub input_tokens: u32,
    pub output_tokens: u32,
}

#[derive(Debug, Clone)]
pub enum State {
    Ready,
    StreamStart,
    StreamStop,
    ThinkingStart,
    ThinkingStop,
    MessageStart,
    MessageStop,
    ToolStart,
    ToolStop,
    Stopped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lifecycle {
    Ready,
    Running,
    WaitingForTools,
    WaitingForInput,
    WaitingForPermission,
    Cancelling,
    Completed,
    Cancelled,
    Failed,
}
impl Lifecycle {
    pub fn terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Cancelled | Self::Failed)
    }
}
