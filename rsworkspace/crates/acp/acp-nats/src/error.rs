use agent_client_protocol::{Error, ErrorCode};
use tracing::warn;
use trogon_nats::NatsError;

pub use crate::constants::AGENT_UNAVAILABLE;

pub fn map_nats_error(e: NatsError) -> Error {
    match &e {
        NatsError::Timeout { subject } => {
            warn!(subject = %subject, "NATS request timed out");
            Error::new(
                ErrorCode::new(AGENT_UNAVAILABLE),
                "Request timed out; agent may be overloaded or unavailable",
            )
        }
        NatsError::Request { subject, error } => {
            warn!(subject = %subject, error = %error, "NATS request failed");
            Error::new(ErrorCode::new(AGENT_UNAVAILABLE), format!("Agent unavailable: {error}"))
        }
        NatsError::Serialize(inner) => {
            warn!(error = %inner, "failed to serialize request");
            Error::new(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to serialize request: {inner}"),
            )
        }
        NatsError::Deserialize(inner) => {
            warn!(error = %inner, "failed to deserialize response");
            Error::new(ErrorCode::INTERNAL_ERROR, "Invalid response from agent")
        }
        NatsError::PublishOperation(inner) => {
            warn!(error = %inner, "publish operation failed");
            Error::new(ErrorCode::INTERNAL_ERROR, "Publish operation failed")
        }
        NatsError::PublishOperationExhausted {
            error: inner,
            subject,
            attempts,
        } => {
            warn!(subject = %subject, error = %inner, attempts, "publish operation exhausted");
            Error::new(ErrorCode::INTERNAL_ERROR, "Publish operation exhausted")
        }
        NatsError::Other(msg) => {
            warn!(error = %msg, "NATS request failed");
            Error::new(ErrorCode::INTERNAL_ERROR, "Request failed")
        }
    }
}

#[cfg(test)]
mod tests;
