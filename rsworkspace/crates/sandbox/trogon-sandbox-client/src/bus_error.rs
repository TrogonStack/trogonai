use trogon_sandbox_protocol::NatsTransferError;

/// A failure of the message bus underneath the client.
#[derive(Debug, thiserror::Error)]
pub enum BusError {
    #[error("NATS request error: {0}")]
    Request(#[from] async_nats::client::RequestError),
    #[error("NATS transfer error: {0}")]
    Transfer(#[from] NatsTransferError),
    #[error("NATS subscribe error: {0}")]
    Subscribe(#[from] async_nats::SubscribeError),
    #[error("bus error: {0}")]
    Custom(#[source] Box<dyn std::error::Error + Send + Sync>),
}
