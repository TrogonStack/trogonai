use async_nats::jetstream::consumer::StreamError;
use async_nats::jetstream::consumer::pull::MessagesError;
use async_nats::jetstream::stream::ConsumerError;
use trogon_sandbox_broker::snapshot_accounting::SnapshotAccountingError;

/// Why tag recovery could not start or had to stop.
#[derive(Debug, thiserror::Error)]
pub enum TagRecoveryError {
    #[error("snapshot publication stream unavailable: {0}")]
    Stream(#[from] SnapshotAccountingError),
    #[error("snapshot tag recovery consumer unavailable: {0}")]
    Consumer(#[from] ConsumerError),
    #[error("snapshot tag recovery consumer has incompatible acknowledgement or retry bounds")]
    IncompatibleConsumer,
    #[error("snapshot tag recovery consumer could not stream messages: {0}")]
    Messages(#[from] StreamError),
    #[error("snapshot tag recovery consumer failed: {0}")]
    Delivery(#[from] MessagesError),
    #[error("snapshot tag recovery consumer closed")]
    Closed,
    #[error("snapshot tag recovery acknowledgement failed: {0}")]
    Acknowledge(#[source] async_nats::Error),
}
