use thiserror::Error;

#[derive(Error, Debug)]
pub enum ClaudeError {
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),
    
    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
    
    #[error("Invalid URL: {0}")]
    InvalidUrl(#[from] url::ParseError),
    
    #[error("Authentication failed: {0}")]
    Authentication(String),
    
    #[error("Rate limit exceeded: {message}")]
    RateLimit { message: String },
    
    #[error("API error: {message} (status: {status})")]
    ApiError { status: u16, message: String },
    
    #[error("Invalid API key")]
    InvalidApiKey,
    
    #[error("Missing required field: {0}")]
    MissingField(String),
    
    #[error("Configuration error: {0}")]
    Configuration(String),
}

pub type Result<T> = std::result::Result<T, ClaudeError>; 