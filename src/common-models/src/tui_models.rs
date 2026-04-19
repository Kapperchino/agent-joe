use commands::command::Command;

#[derive(Debug, Clone)]
pub enum ActorToTui {
    StateChanged(State),
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
