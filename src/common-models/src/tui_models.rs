use crate::runtime_ids::{OperationId, TurnId};
use commands::command::Command;

pub struct ActorToTui {
    pub actor_id: u64,
    pub packet: ActorToTuiPacket,
}
#[derive(Debug, Clone)]
pub enum ActorToTuiPacket {
    SessionChanged,
    SessionError(String),
    SessionChoices(Result<Vec<SessionSummary>, String>),
    SessionResumed(Result<SessionTranscript, String>),
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

#[derive(Debug, Clone)]
pub struct SessionSummary {
    pub id: String,
    pub title: String,
    pub preview: String,
    pub updated_at: Option<std::time::SystemTime>,
    pub status: Lifecycle,
}

#[derive(Debug, Clone)]
pub struct SessionTranscript {
    pub id: String,
    pub messages: Vec<SessionMessage>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionMessage {
    User(String),
    Assistant(String),
    Tool(String),
    Thinking(String),
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Lifecycle {
    Ready,
    Running,
    WaitingForTools,
    WaitingForInput,
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
