use std::num::NonZeroU32;
use std::time::Duration;

use tracing::warn;
use trogon_nats::jetstream::{JetStreamPublisher, PublishOutcome, publish_event};

use crate::audit_sink::AuditSink;
use crate::classify_publish_error::ClassifyPublishError;
use crate::constants::{DEFAULT_ACK_TIMEOUT, DEFAULT_MAX_PUBLISH_ATTEMPTS, LOG_TARGET};
use crate::sealed_record::SealedRecord;
use crate::sink_error::SinkError;

/// Publishes sealed records to JetStream and waits for the `PubAck`.
///
/// `Nats-Msg-Id` is the record id, so a timeout is retried under the same message
/// id and the stream's duplicate window keeps a lost ack from storing the record
/// twice. Only a timeout is retried, and only up to `max_attempts` publishes: an
/// unreachable or full stream returns at once, because waiting does not change
/// the answer inside one request.
#[derive(Debug, Clone)]
pub struct JetStreamSink<P> {
    publisher: P,
    ack_timeout: Duration,
    max_attempts: NonZeroU32,
}

impl<P> JetStreamSink<P> {
    pub fn new(publisher: P, ack_timeout: Duration, max_attempts: NonZeroU32) -> Self {
        Self {
            publisher,
            ack_timeout,
            max_attempts,
        }
    }

    pub fn with_defaults(publisher: P) -> Self {
        Self::new(
            publisher,
            DEFAULT_ACK_TIMEOUT,
            NonZeroU32::new(DEFAULT_MAX_PUBLISH_ATTEMPTS).unwrap_or(NonZeroU32::MIN),
        )
    }
}

impl<P> AuditSink for JetStreamSink<P>
where
    P: JetStreamPublisher,
    P::PublishError: ClassifyPublishError,
{
    async fn publish(&self, record: &SealedRecord) -> Result<(), SinkError> {
        for attempt in 1..=self.max_attempts.get() {
            let outcome = publish_event(
                &self.publisher,
                record.subject().to_owned(),
                record.headers(),
                record.payload().clone(),
                self.ack_timeout,
            )
            .await;
            let Some(error) = failure_of(outcome) else {
                return Ok(());
            };
            warn!(
                target: LOG_TARGET,
                record_id = %record.id(),
                reason = %error.reason(),
                attempt,
                "audit publish was not acknowledged"
            );
            if error != SinkError::Timeout {
                return Err(error);
            }
        }
        Err(SinkError::Timeout)
    }
}

fn failure_of<E>(outcome: PublishOutcome<E>) -> Option<SinkError>
where
    E: ClassifyPublishError + std::fmt::Display,
{
    match outcome {
        PublishOutcome::Published => None,
        PublishOutcome::PublishFailed(error) | PublishOutcome::AckFailed(error) => Some(error.classify()),
        PublishOutcome::AckTimedOut(_) => Some(SinkError::Timeout),
        PublishOutcome::StoreFailed(_) => Some(SinkError::Unreachable),
    }
}

#[cfg(test)]
mod tests;
