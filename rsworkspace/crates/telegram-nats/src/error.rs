//! Error types for telegram-nats

use thiserror::Error;

/// Result type alias
pub type Result<T> = std::result::Result<T, Error>;

/// Error types for telegram-nats operations
#[derive(Debug, Error)]
pub enum Error {
    #[error("NATS error: {0}")]
    Nats(#[from] async_nats::Error),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("Connection error: {0}")]
    Connection(String),

    #[error("Subject error: {0}")]
    Subject(String),

    #[error("Publish error: {0}")]
    Publish(String),

    #[error("Subscribe error: {0}")]
    Subscribe(String),

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Other error: {0}")]
    Other(#[from] anyhow::Error),
}
