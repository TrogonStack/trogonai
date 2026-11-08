use std::fmt;
use std::time::Duration;

use async_nats::jetstream::publish::PublishAck;
use bytes::Bytes;
use tracing::error;

use crate::jetstream::JetStreamPublisher;

#[derive(Debug)]
pub enum PublishOutcome<E: fmt::Display> {
    /// The stream stored the message. The ack names the stream and the sequence
    /// it stored it at, and whether it was a duplicate of a message already
    /// stored under the same `Nats-Msg-Id`.
    Published(PublishAck),
    PublishFailed(E),
    AckFailed(E),
    AckTimedOut(Duration),
    StoreFailed(Box<dyn std::error::Error + Send + Sync>),
}

impl<E: fmt::Display> PublishOutcome<E> {
    pub fn is_ok(&self) -> bool {
        matches!(self, PublishOutcome::Published(_))
    }

    pub fn log_on_error(&self, source_name: &str) {
        match self {
            PublishOutcome::Published(_) => {}
            PublishOutcome::PublishFailed(e) => {
                error!(error = %e, source = source_name, "Failed to publish event to NATS");
            }
            PublishOutcome::AckFailed(e) => {
                error!(error = %e, source = source_name, "NATS ack failed");
            }
            PublishOutcome::AckTimedOut(timeout) => {
                error!(?timeout, source = source_name, "NATS ack timed out");
            }
            PublishOutcome::StoreFailed(e) => {
                error!(error = %e, source = source_name, "Failed to store claim check payload");
            }
        }
    }
}

pub async fn publish_event<P: JetStreamPublisher>(
    js: &P,
    subject: String,
    headers: async_nats::HeaderMap,
    body: Bytes,
    ack_timeout: Duration,
) -> PublishOutcome<P::PublishError> {
    let ack_future = match js.publish_with_headers(subject, headers, body).await {
        Ok(f) => f,
        Err(e) => return PublishOutcome::PublishFailed(e),
    };

    match tokio::time::timeout(ack_timeout, ack_future).await {
        Ok(Ok(ack)) => PublishOutcome::Published(ack),
        Ok(Err(e)) => PublishOutcome::AckFailed(e),
        Err(_) => PublishOutcome::AckTimedOut(ack_timeout),
    }
}

#[cfg(test)]
mod tests;
