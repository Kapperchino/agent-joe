use crate::error::{ClaudeError, Result};
use std::time::Duration;
use url::Url;

#[derive(Debug, Clone)]
pub struct ClaudeConfig {
    pub api_key: String,
    pub base_url: Url,
    pub timeout: Duration,
    pub max_retries: u32,
    pub anthropic_version: String,
}

impl ClaudeConfig {
    pub fn new(api_key: impl Into<String>) -> Result<Self> {
        let api_key = api_key.into();
        if api_key.is_empty() {
            return Err(ClaudeError::InvalidApiKey);
        }

        Ok(Self {
            api_key,
            base_url: Url::parse("https://api.anthropic.com/v1/")?,
            timeout: Duration::from_secs(120),
            max_retries: 3,
            anthropic_version: "2023-06-01".to_string(),
        })
    }

    pub fn with_base_url(mut self, url: impl AsRef<str>) -> Result<Self> {
        self.base_url = Url::parse(url.as_ref())?;
        Ok(self)
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn with_max_retries(mut self, max_retries: u32) -> Self {
        self.max_retries = max_retries;
        self
    }

    pub fn with_anthropic_version(mut self, version: impl Into<String>) -> Self {
        self.anthropic_version = version.into();
        self
    }
}

impl Default for ClaudeConfig {
    fn default() -> Self {
        Self::new("").unwrap_or_else(|_| {
            // This should never happen with an empty string, but just in case
            Self {
                api_key: String::new(),
                base_url: Url::parse("https://api.anthropic.com/v1/").unwrap(),
                timeout: Duration::from_secs(120),
                max_retries: 3,
                anthropic_version: "2023-06-01".to_string(),
            }
        })
    }
} 