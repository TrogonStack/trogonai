use std::time::Duration;

use trogon_sandbox_contract::CredentialError;

/// Why a credential broker request did not produce a result.
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    /// The broker answered and refused the request.
    #[error("credential broker error: {0}")]
    Broker(#[from] CredentialError),
    /// The broker did not answer within the client's timeout.
    #[error("broker timeout after {0:?}")]
    Timeout(Duration),
    /// The request could not be delivered or no reply came back.
    #[error("transport: {0}")]
    Transport(#[source] Box<dyn std::error::Error + Send + Sync>),
    /// The request could not be encoded.
    #[error("encode request: {0}")]
    Encode(#[source] serde_json::Error),
    /// The reply was not a valid broker reply.
    #[error("decode reply: {0}")]
    Decode(#[source] serde_json::Error),
}
