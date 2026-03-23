use crate::claude_config::ClaudeConfig;
use crate::openai_config::OpenAIConfig;
use figment::Figment;
use figment::providers::{Env, Format, Toml};
use serde::{Deserialize, Serialize};

pub const CONFIG_PATH: &str = "~/.turbo-code/config.toml";

#[derive(Debug, Serialize, Deserialize)]
pub enum Config {
    Claude(ClaudeConfig),
    OpenAI(OpenAIConfig),
}

impl Config {
    pub fn new() -> anyhow::Result<Config> {
        let figment: Config = Figment::new().merge(Toml::file(CONFIG_PATH)).extract()?;
        Ok(figment)
    }
}
