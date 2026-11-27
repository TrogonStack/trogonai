use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use futures::stream::BoxStream;

use crate::{BusError, BusMessage};

/// The request and subscribe operations the client needs from a message bus.
///
/// [`NatsBus`](crate::NatsBus) is the production implementation. Tests and embedders can supply their own through
/// [`SandboxClient::from_bus`](crate::SandboxClient::from_bus).
#[async_trait]
pub trait Bus: Send + Sync {
    /// Sends a request and waits up to `timeout` for the reply. `Ok(None)` means no reply arrived in time.
    async fn request(
        &self,
        subject: &str,
        payload: Bytes,
        timeout: Duration,
        headers: Option<async_nats::HeaderMap>,
    ) -> Result<Option<Bytes>, BusError>;

    /// Subscribes to `subject` and streams every message published on it.
    async fn subscribe(&self, subject: &str) -> Result<BoxStream<'static, BusMessage>, BusError>;
}
