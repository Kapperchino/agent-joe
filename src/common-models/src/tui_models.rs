#[derive(Debug, Clone)]
pub enum ActorToTui {
    StateChanged(State),
    Data(String),
    ToolUse(Vec<String>),
    CommandResult(Command, String),
    TokensUpdated(TokenCount),
}

#[derive(Debug, Clone)]
pub enum Command {
    PrintContext,
}

impl Command {
    pub fn parse(string: &str) -> anyhow::Result<Command> {
        match string {
            "context" => Ok(Command::PrintContext),
            _ => Err(anyhow::anyhow!("Invalid command")),
        }
    }
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
