//! # Claude API Client
//! 
//! A comprehensive Rust client for the Anthropic Claude API.
//! 
//! ## Features
//! 
//! - Support for Claude's Messages API
//! - Streaming responses
//! - Tool/function calling
//! - Image support (base64)
//! - Comprehensive error handling
//! - Configurable client settings
//! - Builder pattern for easy request construction
//! 
//! ## Quick Start
//! 
//! ```rust,no_run
//! use claude_api::{ClaudeClient, ClaudeConfig, Message};
//! 
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let client = ClaudeClient::from_api_key("your-api-key")?;
//!     
//!     let messages = vec![
//!         Message::user("Hello, Claude!")
//!     ];
//!     
//!     let response = client
//!         .create_message_simple("claude-3-sonnet-20240229", 1000, messages)
//!         .await?;
//!     
//!     println!("Claude says: {:?}", response.content);
//!     Ok(())
//! }
//! ```
//! 
//! ## Advanced Usage
//! 
//! ```rust,no_run
//! use claude_api::{ClaudeClient, ClaudeConfig, MessageBuilder, Message, Tool};
//! use std::time::Duration;
//! 
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let config = ClaudeConfig::new("your-api-key")?
//!         .with_timeout(Duration::from_secs(60))
//!         .with_anthropic_version("2023-06-01");
//!     
//!     let client = ClaudeClient::new(config)?;
//!     
//!     let request = MessageBuilder::new("claude-3-sonnet-20240229", 1000)
//!         .system("You are a helpful assistant")
//!         .user_message("What's the weather like?")
//!         .temperature(0.7)
//!         .build();
//!     
//!     let response = client.create_message(request).await?;
//!     Ok(())
//! }
//! ```

pub mod client;
pub mod config;
pub mod error;
pub mod models;

// Re-export the main types for convenience
pub use client::{ClaudeClient, MessageBuilder};
pub use config::ClaudeConfig;
pub use error::{ClaudeError, Result};
pub use models::{
    ContentBlock, CreateMessageRequest, CreateMessageResponse, ImageSource, Message, StreamEvent,
    Tool, Usage,
};

// Common model constants
pub mod models_constants {
    /// Claude 3 Opus model identifier
    pub const CLAUDE_3_OPUS: &str = "claude-3-opus-20240229";
    
    /// Claude 3 Sonnet model identifier  
    pub const CLAUDE_3_SONNET: &str = "claude-3-sonnet-20240229";
    
    /// Claude 3 Haiku model identifier
    pub const CLAUDE_3_HAIKU: &str = "claude-3-haiku-20240307";
    
    /// Claude 3.5 Sonnet model identifier
    pub const CLAUDE_3_5_SONNET: &str = "claude-3-5-sonnet-20241022";
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_message_constructors() {
        let user_msg = Message::user("Hello");
        assert_eq!(user_msg.role, "user");
        
        let assistant_msg = Message::assistant("Hi there");
        assert_eq!(assistant_msg.role, "assistant");
    }

    #[test]
    fn test_config_creation() {
        let config = ClaudeConfig::new("test-key").unwrap();
        assert_eq!(config.api_key, "test-key");
        assert_eq!(config.anthropic_version, "2023-06-01");
    }

    #[test]
    fn test_config_invalid_key() {
        let result = ClaudeConfig::new("");
        assert!(result.is_err());
    }

    #[test]
    fn test_message_builder() {
        let request = MessageBuilder::new("claude-3-sonnet-20240229", 1000)
            .system("You are helpful")
            .user_message("Hello")
            .temperature(0.7)
            .build();
        
        assert_eq!(request.model, "claude-3-sonnet-20240229");
        assert_eq!(request.max_tokens, 1000);
        assert!(request.system.is_some());
        assert_eq!(request.messages.len(), 1);
        assert_eq!(request.temperature, Some(0.7));
    }
} 