use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use futures::stream::BoxStream;
use trogon_sandbox_client::{Bus, BusError, BusMessage};

/// Lets one shared [`Bus`] back every per-call client.
pub(crate) struct SharedBus(pub(crate) Arc<dyn Bus>);

#[async_trait]
impl Bus for SharedBus {
    async fn request(
        &self,
        subject: &str,
        payload: Bytes,
        timeout: Duration,
        headers: Option<async_nats::HeaderMap>,
    ) -> Result<Option<Bytes>, BusError> {
        self.0.request(subject, payload, timeout, headers).await
    }

    async fn subscribe(&self, subject: &str) -> Result<BoxStream<'static, BusMessage>, BusError> {
        self.0.subscribe(subject).await
    }
}
