use async_trait::async_trait;
use bytes::Bytes;

/// Where output frames go. The worker publishes on core NATS; tests collect.
#[async_trait]
pub trait FrameSink: Send + Sync {
    /// Delivers one frame on `subject`.
    async fn publish(&self, subject: String, payload: Bytes);
}

#[async_trait]
impl FrameSink for async_nats::Client {
    async fn publish(&self, subject: String, payload: Bytes) {
        if let Err(error) = async_nats::Client::publish(self, subject, payload).await {
            tracing::warn!(%error, "terminal output publish failed");
        }
    }
}
