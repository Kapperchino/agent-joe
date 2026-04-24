use strum_macros::{EnumString, VariantNames};

#[derive(Debug, PartialEq, EnumString, VariantNames, Clone)]
pub enum OpenAIModels {
    #[strum(serialize = "gpt-5.5")]
    GPT5_5,
    #[strum(serialize = "gpt-5.4")]
    GPT5_4,
    #[strum(serialize = "gpt-5.3-codex")]
    GPT5_3_Codex,
}
