use agent_client_protocol::{Error, ErrorCode};
use tracing::warn;
use trogon_nats::NatsError;

pub use crate::constants::AGENT_UNAVAILABLE;

pub fn map_nats_error(e: NatsError) -> Error {
    match &e {
        NatsError::Timeout { subject } => {
            warn!(subject = %subject, "NATS request timed out");
            Error::new(
                ErrorCode::Other(AGENT_UNAVAILABLE).into(),
                "Request timed out; agent may be overloaded or unavailable",
            )
        }
        NatsError::Request { subject, error } => {
            warn!(subject = %subject, error = %error, "NATS request failed");
            Error::new(
                ErrorCode::Other(AGENT_UNAVAILABLE).into(),
                format!("Agent unavailable: {error}"),
            )
        }
        NatsError::Serialize(inner) => {
            warn!(error = %inner, "failed to serialize request");
            Error::new(
                ErrorCode::InternalError.into(),
                format!("Failed to serialize request: {inner}"),
            )
        }
        NatsError::Deserialize(inner) => {
            warn!(error = %inner, "failed to deserialize response");
            Error::new(
                ErrorCode::InternalError.into(),
                "Invalid response from agent",
            )
        }
        NatsError::PublishOperation(inner) => {
            warn!(error = %inner, "publish operation failed");
            Error::new(ErrorCode::InternalError.into(), "Publish operation failed")
        }
        NatsError::PublishOperationExhausted {
            error: inner,
            subject,
            attempts,
        } => {
            warn!(subject = %subject, error = %inner, attempts, "publish operation exhausted");
            Error::new(
                ErrorCode::InternalError.into(),
                "Publish operation exhausted",
            )
        }
        NatsError::Other(msg) => {
            warn!(error = %msg, "NATS request failed");
            Error::new(ErrorCode::InternalError.into(), "Request failed")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::ErrorCode;
    use trogon_nats::{NatsError, PublishOperationError};

    #[test]
    fn timeout_returns_agent_unavailable() {
        let err = map_nats_error(NatsError::Timeout {
            subject: "test.subject".into(),
        });
        assert!(err.to_string().contains("timed out"));
        assert_eq!(err.code, ErrorCode::Other(AGENT_UNAVAILABLE));
    }

    #[test]
    fn request_error_returns_agent_unavailable() {
        let err = map_nats_error(NatsError::Request {
            subject: "test.subject".into(),
            error: "connection refused".into(),
        });
        assert!(err.to_string().contains("Agent unavailable"));
        assert_eq!(err.code, ErrorCode::Other(AGENT_UNAVAILABLE));
    }

    #[test]
    fn serialize_error_returns_internal() {
        let serde_err = serde_json::to_vec(&FailsSerialize).unwrap_err();
        let err = map_nats_error(NatsError::Serialize(serde_err));
        assert!(err.to_string().contains("serialize"));
        assert_eq!(err.code, ErrorCode::InternalError);
    }

    #[test]
    fn deserialize_error_returns_internal() {
        let serde_err = serde_json::from_str::<serde_json::Value>("invalid").unwrap_err();
        let err = map_nats_error(NatsError::Deserialize(serde_err));
        assert!(err.to_string().contains("Invalid response from agent"));
        assert_eq!(err.code, ErrorCode::InternalError);
    }

    #[test]
    fn publish_operation_returns_internal() {
        let err = map_nats_error(NatsError::PublishOperation(PublishOperationError(
            "flush failed".into(),
        )));
        assert!(err.to_string().contains("Publish operation failed"));
        assert_eq!(err.code, ErrorCode::InternalError);
    }

    #[test]
    fn publish_operation_exhausted_returns_internal() {
        let err = map_nats_error(NatsError::PublishOperationExhausted {
            error: PublishOperationError("flush failed".into()),
            subject: "test.subject".into(),
            attempts: 3,
        });
        assert!(err.to_string().contains("Publish operation exhausted"));
        assert_eq!(err.code, ErrorCode::InternalError);
    }

    #[test]
    fn other_error_returns_internal() {
        let err = map_nats_error(NatsError::Other("misc failure".into()));
        assert!(err.to_string().contains("Request failed"));
        assert_eq!(err.code, ErrorCode::InternalError);
    }

    struct FailsSerialize;
    impl serde::Serialize for FailsSerialize {
        fn serialize<S: serde::Serializer>(&self, _s: S) -> Result<S::Ok, S::Error> {
            Err(serde::ser::Error::custom("test serialize failure"))
        }
    }
}
