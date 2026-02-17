//! Error types for discord-nats

use thiserror::Error;

/// Result type alias
pub type Result<T> = std::result::Result<T, Error>;

/// Error types for discord-nats operations
#[derive(Debug, Error)]
pub enum Error {
    #[error("NATS error: {0}")]
    Nats(#[from] async_nats::Error),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("Connection error: {0}")]
    Connection(String),

    #[error("Publish error: {0}")]
    Publish(String),

    #[error("Subscribe error: {0}")]
    Subscribe(String),

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Other error: {0}")]
    Other(#[from] anyhow::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_connection_error_display() {
        let err = Error::Connection("refused".to_string());
        assert_eq!(err.to_string(), "Connection error: refused");
    }

    #[test]
    fn test_publish_error_display() {
        let err = Error::Publish("broker unavailable".to_string());
        assert_eq!(err.to_string(), "Publish error: broker unavailable");
    }

    #[test]
    fn test_subscribe_error_display() {
        let err = Error::Subscribe("no permission".to_string());
        assert_eq!(err.to_string(), "Subscribe error: no permission");
    }

    #[test]
    fn test_config_error_display() {
        let err = Error::Config("missing credentials".to_string());
        assert_eq!(err.to_string(), "Configuration error: missing credentials");
    }

    #[test]
    fn test_from_serde_json_error() {
        let json_err: serde_json::Error = serde_json::from_str::<i32>("not_a_number").unwrap_err();
        let err: Error = json_err.into();
        assert!(err.to_string().starts_with("Serialization error:"));
    }

    #[test]
    fn test_from_anyhow_error() {
        let anyhow_err = anyhow::anyhow!("something went wrong");
        let err: Error = anyhow_err.into();
        assert_eq!(err.to_string(), "Other error: something went wrong");
    }

    #[test]
    fn test_result_ok() {
        let r: Result<i32> = Ok(42);
        assert!(r.is_ok());
    }

    #[test]
    fn test_result_err() {
        let r: Result<i32> = Err(Error::Connection("fail".to_string()));
        assert!(r.is_err());
    }
}
