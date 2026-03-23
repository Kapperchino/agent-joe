use crate::claude_config::ClaudeConfig;
use crate::openai_codex_auth::refresh_codex_tokens;
use crate::openai_config::OpenAIConfig;
use anyhow::Context;
use figment::Figment;
use figment::providers::{Format, Toml};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tokio::fs;

pub const CONFIG_DIR_NAME: &str = ".turbo-code";
pub const CONFIG_FILE_NAME: &str = "config.toml";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Config {
    Claude(ClaudeConfig),
    OpenAI(OpenAIConfig),
}

impl Config {
    pub fn new() -> anyhow::Result<Config> {
        let path = Self::path()?;
        let figment: Config = Figment::new().merge(Toml::file(&path)).extract()?;
        Ok(figment)
    }

    pub async fn delete() -> anyhow::Result<()> {
        let path = Self::path()?;
        fs::remove_file(path).await?;
        Ok(())
    }

    pub fn load_optional() -> anyhow::Result<Option<Config>> {
        let path = Self::path()?;
        if !path.exists() {
            return Ok(None);
        }
        Self::new().map(Some)
    }

    pub async fn save(&self) -> anyhow::Result<()> {
        let path = Self::path()?;
        let parent = path
            .parent()
            .context("Config path should always have a parent directory")?;
        fs::create_dir_all(parent).await?;
        fs::write(path, toml::to_string_pretty(self)?).await?;
        Ok(())
    }

    pub fn path() -> anyhow::Result<PathBuf> {
        let home_dir = dirs::home_dir().context("Failed to determine the home directory")?;
        Ok(home_dir.join(CONFIG_DIR_NAME).join(CONFIG_FILE_NAME))
    }

    pub async fn prepare(mut self) -> anyhow::Result<Config> {
        if let Config::OpenAI(OpenAIConfig { auth, .. }) = &mut self {
            if let crate::openai_config::OpenAIAuthConfig::Codex(codex) = auth {
                if refresh_codex_tokens(codex).await? {
                    self.save().await?;
                }
            }
        }

        Ok(self)
    }
}
