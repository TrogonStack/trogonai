use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Error)]
pub enum Error {
    #[error("NATS error: {0}")]
    Nats(#[from] async_nats::Error),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("Subject error: {0}")]
    Subject(String),

    #[error("Publish error: {0}")]
    Publish(String),

    #[error("Subscribe error: {0}")]
    Subscribe(String),

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("JetStream setup error: {0}")]
    Setup(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_subject_error_display() {
        let err = Error::Subject("invalid subject".to_string());
        assert_eq!(err.to_string(), "Subject error: invalid subject");
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
    fn test_setup_error_display() {
        let err = Error::Setup("stream creation failed".to_string());
        assert_eq!(err.to_string(), "JetStream setup error: stream creation failed");
    }

    #[test]
    fn test_from_serde_json_error() {
        let json_err: serde_json::Error = serde_json::from_str::<i32>("not_a_number").unwrap_err();
        let err: Error = json_err.into();
        assert!(err.to_string().starts_with("Serialization error:"));
    }

    #[test]
    fn test_result_ok() {
        let r: Result<i32> = Ok(42);
        if let Ok(v) = r {
            assert_eq!(v, 42);
        } else {
            panic!("should be ok");
        }
    }

    #[test]
    fn test_result_err() {
        let r: Result<i32> = Err(Error::Setup("fail".to_string()));
        if let Err(e) = r {
            assert!(e.to_string().contains("JetStream setup error"));
        } else {
            panic!("should be err");
        }
    }
}
