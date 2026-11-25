use std::num::NonZeroU32;
use std::time::Duration;

use async_nats::HeaderMap;
use bytes::Bytes;
use tracing::warn;
use trogon_nats::jetstream::{JetStreamPublisher, PublishOutcome, publish_event};

use crate::audit_sink::AuditSink;
use crate::checkpoint_sink::CheckpointSink;
use crate::classify_publish_error::ClassifyPublishError;
use crate::constants::{DEFAULT_ACK_TIMEOUT, DEFAULT_MAX_PUBLISH_ATTEMPTS, LOG_TARGET};
use crate::sealed_record::SealedRecord;
use crate::signed_checkpoint::SignedCheckpoint;
use crate::sink_error::SinkError;
use crate::stored_ack::StoredAck;

/// Publishes sealed records and signed checkpoints to JetStream and waits for
/// the `PubAck`.
///
/// `Nats-Msg-Id` is the record or checkpoint id, so a timeout is retried under
/// the same message id and the stream's duplicate window keeps a lost ack from
/// storing it twice. Only a timeout is retried, and only up to `max_attempts`
/// publishes: an unreachable or full stream returns at once, because waiting
/// does not change the answer inside one request.
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

impl<P> JetStreamSink<P>
where
    P: JetStreamPublisher,
    P::PublishError: ClassifyPublishError,
{
    /// Publishes one message, retrying only a timeout and under the same headers,
    /// so the same message id. `unacknowledged` logs each attempt that failed.
    async fn publish_message(
        &self,
        subject: &str,
        headers: &HeaderMap,
        payload: &Bytes,
        unacknowledged: impl Fn(SinkError, u32),
    ) -> Result<StoredAck, SinkError> {
        for attempt in 1..=self.max_attempts.get() {
            let outcome = publish_event(
                &self.publisher,
                subject.to_owned(),
                headers.clone(),
                payload.clone(),
                self.ack_timeout,
            )
            .await;
            let error = match stored(outcome) {
                Ok(ack) => return Ok(ack),
                Err(error) => error,
            };
            unacknowledged(error, attempt);
            if error != SinkError::Timeout {
                return Err(error);
            }
        }
        Err(SinkError::Timeout)
    }
}

impl<P> AuditSink for JetStreamSink<P>
where
    P: JetStreamPublisher,
    P::PublishError: ClassifyPublishError,
{
    async fn publish(&self, record: &SealedRecord) -> Result<StoredAck, SinkError> {
        self.publish_message(
            record.subject(),
            &record.headers(),
            record.payload(),
            |error, attempt| {
                warn!(
                    target: LOG_TARGET,
                    record_id = %record.id(),
                    reason = %error.reason(),
                    attempt,
                    "audit publish was not acknowledged"
                );
            },
        )
        .await
    }
}

impl<P> CheckpointSink for JetStreamSink<P>
where
    P: JetStreamPublisher,
    P::PublishError: ClassifyPublishError,
{
    async fn publish_checkpoint(&self, checkpoint: &SignedCheckpoint) -> Result<StoredAck, SinkError> {
        self.publish_message(
            checkpoint.subject(),
            &checkpoint.headers(),
            checkpoint.payload(),
            |error, attempt| {
                warn!(
                    target: LOG_TARGET,
                    checkpoint_id = %checkpoint.id(),
                    reason = %error.reason(),
                    attempt,
                    "audit checkpoint publish was not acknowledged"
                );
            },
        )
        .await
    }
}

fn stored<E>(outcome: PublishOutcome<E>) -> Result<StoredAck, SinkError>
where
    E: ClassifyPublishError + std::fmt::Display,
{
    match outcome {
        PublishOutcome::Published(ack) => Ok(StoredAck::from(ack)),
        PublishOutcome::PublishFailed(error) | PublishOutcome::AckFailed(error) => Err(error.classify()),
        PublishOutcome::AckTimedOut(_) => Err(SinkError::Timeout),
        PublishOutcome::StoreFailed(_) => Err(SinkError::Unreachable),
    }
}

#[cfg(test)]
mod tests;
