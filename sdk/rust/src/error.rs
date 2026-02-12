//! Error types for the SDK

use thiserror::Error;

/// SDK Result type
pub type Result<T> = std::result::Result<T, Error>;

/// SDK Error types
#[derive(Error, Debug)]
pub enum Error {
    /// RPC communication error
    #[error("RPC error: {0}")]
    RpcError(String),
    
    /// HTTP request error
    #[error("HTTP error: {0}")]
    HttpError(#[from] reqwest::Error),
    
    /// Parse error
    #[error("Parse error: {0}")]
    ParseError(String),
    
    /// Transaction build error
    #[error("Build error: {0}")]
    BuildError(String),
    
    /// Configuration error
    #[error("Config error: {0}")]
    ConfigError(String),
    
    /// Serialization error
    #[error("Serialization error: {0}")]
    SerializationError(String),
}
