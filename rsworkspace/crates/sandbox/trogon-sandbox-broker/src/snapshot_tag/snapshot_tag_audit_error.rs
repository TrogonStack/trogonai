use async_nats::jetstream;

/// Why emitting or reading snapshot tag audit events failed.
#[derive(Debug, thiserror::Error)]
pub enum SnapshotTagAuditError {
    #[error("snapshot tag audit stream unavailable: {0}")]
    CreateStream(#[from] jetstream::context::CreateStreamError),
    #[error("snapshot tag audit event publish failed: {0}")]
    Publish(#[from] jetstream::context::PublishError),
    #[error("snapshot tag audit event is not valid JSON: {0}")]
    Encoding(#[from] serde_json::Error),
    #[error("open audit stream: {0}")]
    OpenStream(#[from] jetstream::context::GetStreamError),
    #[error("create audit consumer: {0}")]
    Consumer(#[from] jetstream::stream::ConsumerError),
    #[error("fetch audit messages: {0}")]
    Fetch(#[from] jetstream::consumer::pull::BatchError),
    #[error("audit message error: {0}")]
    Message(#[from] async_nats::Error),
}
