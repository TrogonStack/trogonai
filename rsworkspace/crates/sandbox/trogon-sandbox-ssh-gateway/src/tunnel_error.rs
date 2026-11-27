use std::time::Duration;

use trogon_sandbox_protocol::DispatchSignError;

/// Why an exchange with the worker over NATS failed.
#[derive(Debug, thiserror::Error)]
pub enum TunnelError {
    #[error("worker rejected attach: {0}")]
    AttachRejected(String),
    #[error("ssh is unsupported for this lease: {0}")]
    Unsupported(String),
    #[error("worker rejected detach: {0}")]
    DetachRejected(String),
    #[error("NATS request timeout after {0:?}")]
    Timeout(Duration),
    #[error("request failed: {0}")]
    Request(#[from] async_nats::RequestError),
    #[error("publish failed: {0}")]
    Publish(#[from] async_nats::PublishError),
    #[error("subscribe failed: {0}")]
    Subscribe(#[from] async_nats::SubscribeError),
    #[error("request could not be signed: {0}")]
    Sign(#[from] DispatchSignError),
    #[error("message could not be encoded: {0}")]
    Encode(#[source] serde_json::Error),
    #[error("reply could not be decoded: {0}")]
    Decode(#[source] serde_json::Error),
}
