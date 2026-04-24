use strum_macros::{EnumString, VariantNames};

#[derive(Debug, PartialEq, EnumString, VariantNames, Clone)]
pub enum ClaudeModels{
    #[strum(serialize = "claude-opus-4-7")]
    Opus4_7,
    #[strum(serialize = "claude-sonnet-4-6")]
    Sonnet4_6,
    #[strum(serialize = "claude-haiku-4-5")]
    Haiku4_5
}