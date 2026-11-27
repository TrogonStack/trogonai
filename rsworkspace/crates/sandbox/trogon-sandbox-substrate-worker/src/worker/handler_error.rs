use async_nats::PublishError;
use trogon_sandbox_protocol::NatsTransferError;

/// Why one request could not be answered.
#[derive(Debug, thiserror::Error)]
pub(crate) enum HandlerError {
    /// A reply could not be encoded.
    #[error("failed to encode the reply: {0}")]
    Encode(#[from] serde_json::Error),
    /// A reply could not be published.
    #[error("failed to publish the reply: {0}")]
    Publish(#[from] PublishError),
    /// A request or reply could not cross the chunked transfer.
    #[error(transparent)]
    Transfer(#[from] NatsTransferError),
}
