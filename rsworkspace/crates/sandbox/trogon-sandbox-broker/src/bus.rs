use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;

#[async_trait]
pub trait BrokerBus: Send + Sync {
    async fn request(&self, subject: &str, payload: Bytes) -> anyhow::Result<Bytes>;
    async fn publish(&self, subject: &str, payload: Bytes) -> anyhow::Result<()>;

    /// Same as `publish`, but tagged with a `Nats-Msg-Id` so a repeat of the
    /// same event is recognisable: a JetStream stream capturing the subject
    /// drops it within its duplicate window, and a subscriber can compare the
    /// header. For events a retry may send twice. Transports without headers
    /// fall back to a plain publish.
    async fn publish_with_msg_id(&self, subject: &str, _msg_id: &str, payload: Bytes) -> anyhow::Result<()> {
        self.publish(subject, payload).await
    }

    /// Same as `request`, but with a caller-chosen deadline instead of the transport's
    /// default. Providers such as the substrate worker can take far longer than a typical
    /// request-reply round trip on a cold golden-snapshot build, so lease dispatch cannot
    /// share a short default timeout with the rest of this trait's callers.
    async fn request_with_timeout(&self, subject: &str, payload: Bytes, _timeout: Duration) -> anyhow::Result<Bytes> {
        self.request(subject, payload).await
    }

    /// Same as `request` (or `request_with_timeout` when `timeout` is set),
    /// carrying the signed envelope `auth` in the `Sandbox-Dispatch-Auth`
    /// header. Transports without headers send the request unsigned.
    async fn request_with_auth(
        &self,
        subject: &str,
        _auth: &str,
        payload: Bytes,
        timeout: Option<Duration>,
    ) -> anyhow::Result<Bytes> {
        match timeout {
            Some(timeout) => self.request_with_timeout(subject, payload, timeout).await,
            None => self.request(subject, payload).await,
        }
    }

    /// Same as `publish`, carrying the signed envelope `auth`.
    async fn publish_with_auth(&self, subject: &str, _auth: &str, payload: Bytes) -> anyhow::Result<()> {
        self.publish(subject, payload).await
    }

    fn max_payload(&self) -> Option<usize> {
        None
    }
}

#[cfg(test)]
mod tests;
