//! Subject-filtered, sequence-ordered JetStream replay.
//!
//! Reads use an ordered JetStream consumer with a `filter_subject` and a
//! `DeliverPolicy::ByStartSequence` instead of scanning every physical
//! sequence with a direct-get, so only messages that match the requested
//! subject (or none, for a full-stream read) cross the wire. The consumer is
//! self-healing for most disconnects (`async_nats`'s `Ordered` stream
//! recreates itself on missed heartbeats, consumer deletion, and
//! no-responders); the residual failure kinds it does not recover from are
//! retried here by recreating the consumer from the last successfully
//! processed sequence, bounded by [`ReplayRetryPolicy`].

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use async_nats::jetstream;
use async_nats::jetstream::consumer::pull;
use async_nats::jetstream::consumer::{DeliverPolicy, ReplayPolicy, StreamErrorKind};
use async_nats::jetstream::stream::ConsumerErrorKind;
use futures::StreamExt;
use opentelemetry::global;
use opentelemetry::metrics::{Counter, Histogram};
use tracing::Instrument;
use trogon_decider_runtime::StreamEvent;
use trogon_semconv::{metric, span};

use super::{ReadStreamError, StreamMessage, StreamStoreError, record_stream_message};

const METER_NAME: &str = "trogon-decider-nats";

struct ReplayMetrics {
    replay_duration: Histogram<f64>,
    replay_retries: Counter<u64>,
}

impl ReplayMetrics {
    fn new() -> Self {
        let meter = global::meter(METER_NAME);
        Self {
            replay_duration: metric::build_decider_replay_duration(&meter),
            replay_retries: metric::build_decider_replay_retries(&meter),
        }
    }
}

static METRICS: OnceLock<ReplayMetrics> = OnceLock::new();

fn metrics() -> &'static ReplayMetrics {
    METRICS.get_or_init(ReplayMetrics::new)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ReplayRetryPolicy {
    max_attempts: u32,
    base_delay: Duration,
    max_delay: Duration,
}

impl ReplayRetryPolicy {
    pub(super) const fn new(max_attempts: u32, base_delay: Duration, max_delay: Duration) -> Self {
        Self {
            max_attempts,
            base_delay,
            max_delay,
        }
    }

    pub(super) fn decide(&self, attempts_used: u32) -> RetryDecision {
        if attempts_used >= self.max_attempts {
            return RetryDecision::GiveUp;
        }
        let exponent = attempts_used.min(31);
        let multiplier = 1u32.checked_shl(exponent).unwrap_or(u32::MAX);
        let delay = self.base_delay.saturating_mul(multiplier).min(self.max_delay);
        RetryDecision::Retry { delay }
    }
}

impl Default for ReplayRetryPolicy {
    fn default() -> Self {
        Self::new(5, Duration::from_millis(100), Duration::from_secs(5))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RetryDecision {
    Retry { delay: Duration },
    GiveUp,
}

pub(super) fn is_empty_replay_range(from_sequence: u64, to_sequence: u64) -> bool {
    from_sequence == 0 || to_sequence == 0 || from_sequence > to_sequence
}

fn replay_consumer_config(filter_subject: Option<&str>, start_sequence: u64) -> pull::OrderedConfig {
    pull::OrderedConfig {
        deliver_policy: DeliverPolicy::ByStartSequence { start_sequence },
        replay_policy: ReplayPolicy::Instant,
        filter_subject: filter_subject.unwrap_or_default().to_string(),
        ..Default::default()
    }
}

pub(super) fn is_transient_replay_error(error: &StreamStoreError) -> bool {
    let StreamStoreError::Read(error) = error else {
        return false;
    };
    match error {
        ReadStreamError::CreateReplayConsumer { source } => {
            matches!(source.kind(), ConsumerErrorKind::TimedOut | ConsumerErrorKind::Request)
        }
        ReadStreamError::OpenReplayMessageStream { source } => matches!(source.kind(), StreamErrorKind::TimedOut),
        ReadStreamError::ReadReplayMessage { source } => matches!(source.kind(), pull::OrderedErrorKind::Pull),
        ReadStreamError::ReplayEndedBeforeTarget { .. } => true,
        _ => false,
    }
}

async fn replay_attempt(
    stream: &jetstream::stream::Stream,
    filter_subject: Option<&str>,
    from_sequence: u64,
    to_sequence: u64,
    stream_id: &mut impl FnMut(&StreamMessage) -> String,
    events: &mut Vec<StreamEvent>,
) -> Result<(), StreamStoreError> {
    let consumer = stream
        .create_consumer(replay_consumer_config(filter_subject, from_sequence))
        .await
        .map_err(|source| ReadStreamError::CreateReplayConsumer { source })?;
    let mut messages = consumer
        .messages()
        .await
        .map_err(|source| ReadStreamError::OpenReplayMessageStream { source })?;

    while let Some(message) = messages.next().await {
        let message = message.map_err(|source| ReadStreamError::ReadReplayMessage { source })?;
        let info = message
            .info()
            .map_err(|source| ReadStreamError::ReadReplayMessageInfo { source })?;
        let sequence = info.stream_sequence;
        if sequence > to_sequence {
            return Ok(());
        }

        let stream_message = StreamMessage {
            subject: message.subject.clone(),
            sequence,
            headers: message.headers.clone().unwrap_or_default(),
            payload: message.payload.clone(),
            time: info.published,
        };
        let id = stream_id(&stream_message);
        events.push(record_stream_message(stream_message, id)?);

        if sequence >= to_sequence {
            return Ok(());
        }
    }

    Err(ReadStreamError::ReplayEndedBeforeTarget { to_sequence }.into())
}

/// Replays events from an ordered, subject-filtered JetStream consumer over
/// the inclusive sequence range `[from_sequence, to_sequence]`.
///
/// `to_sequence` is a snapshot bound taken by the caller before the replay
/// starts; the replay never reads past it, even if the stream keeps growing
/// while the consumer is open. A transient failure (a reconnect, a timeout,
/// the message stream ending early) recreates the consumer starting from the
/// sequence right after the last event this call already produced, bounded
/// by `retry_policy`. Any other failure propagates immediately.
pub(super) async fn replay_ordered_range(
    stream: &jetstream::stream::Stream,
    filter_subject: Option<&str>,
    from_sequence: u64,
    to_sequence: u64,
    retry_policy: ReplayRetryPolicy,
    mut stream_id: impl FnMut(&StreamMessage) -> String,
) -> Result<Vec<StreamEvent>, StreamStoreError> {
    let span = tracing::info_span!(
        span::DECIDER_REPLAY_STREAM,
        otel.kind = "client",
        stream_id = filter_subject.unwrap_or_default(),
    );
    let start = Instant::now();

    let result = async move {
        if is_empty_replay_range(from_sequence, to_sequence) {
            return Ok(Vec::new());
        }

        let mut events = Vec::new();
        let mut next_sequence = from_sequence;
        let mut attempts_used = 0u32;

        loop {
            let Err(error) = replay_attempt(
                stream,
                filter_subject,
                next_sequence,
                to_sequence,
                &mut stream_id,
                &mut events,
            )
            .await
            else {
                return Ok(events);
            };

            next_sequence = events
                .last()
                .map_or(from_sequence, |event| event.stream_position.as_u64().saturating_add(1));

            if !is_transient_replay_error(&error) {
                return Err(error);
            }

            match retry_policy.decide(attempts_used) {
                RetryDecision::Retry { delay } => {
                    attempts_used = attempts_used.saturating_add(1);
                    metrics().replay_retries.add(1, &[]);
                    tokio::time::sleep(delay).await;
                }
                RetryDecision::GiveUp => return Err(error),
            }
        }
    }
    .instrument(span)
    .await;

    metrics().replay_duration.record(start.elapsed().as_secs_f64(), &[]);
    result
}

#[cfg(test)]
mod tests;
