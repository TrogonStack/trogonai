//! Push-DLQ mirror.
//!
//! A pull-consumer that observes `{prefix}.push.dlq.>` and republishes each
//! envelope to `{prefix}.push.dlq.mirror.<caller_id>.<task_id>` so a tenant
//! audit surface can subscribe to its own DLQ without needing access to the
//! authoritative stream. Loop markers (`X-A2a-Dlq-Mirrored` header and the
//! `.mirror.` subject infix) plus a `PushDlqDedupGate` keep redelivery from
//! producing duplicate mirror publishes.

use std::sync::Arc;
use std::time::Duration;

use a2a_nats::A2aPrefix;
use a2a_nats::constants::NATS_MSG_ID_HEADER;
use a2a_nats::push::dlq_dedup::PushDlqDedupGate;
use a2a_nats::push::push_idempotency_key::PushIdempotencyKey;
use async_nats::HeaderMap;
use async_nats::jetstream::consumer::pull::Config;
use async_nats::jetstream::consumer::{AckPolicy, DeliverPolicy, ReplayPolicy};
use bytes::Bytes;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use trogon_nats::jetstream::JetStreamPublisher;
use trogon_std::env::ReadEnv;

// Only the real `run_push_dlq_mirror` uses these — the cfg(coverage) stub
// returns immediately and would otherwise carry unused-import warnings.
#[cfg(not(coverage))]
use a2a_nats::nats::subjects::A2aStream;
#[cfg(not(coverage))]
use futures::StreamExt;

pub const ENV_PUSH_DLQ_MIRROR: &str = "A2A_GATEWAY_PUSH_DLQ_MIRROR";
pub const ENV_PUSH_DLQ_MIRROR_DURABLE: &str = "A2A_GATEWAY_PUSH_DLQ_DURABLE";
pub const PUSH_DLQ_MIRROR_HEADER: &str = "X-A2a-Dlq-Mirrored";

const MAX_PUBLISH_RETRIES: u32 = 3;
const RETRY_BASE_DELAY: Duration = Duration::from_millis(100);

/// Validated NATS durable name for the mirror's pull consumer.
///
/// NATS rejects durables with whitespace or punctuation outside the
/// `[A-Za-z0-9-_]` set; carrying the validation in the type stops a
/// misconfigured env var from reaching the JetStream client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushDlqMirrorDurable(String);

#[derive(Debug, thiserror::Error)]
pub enum PushDlqMirrorDurableError {
    #[error("push DLQ mirror durable name must be non-empty ASCII alphanumeric, '-', or '_'")]
    Invalid,
}

impl PushDlqMirrorDurable {
    pub const DEFAULT: &'static str = "a2a-gateway-push-dlq-mirror";

    pub fn new(raw: impl AsRef<str>) -> Result<Self, PushDlqMirrorDurableError> {
        let trimmed = raw.as_ref().trim();
        if trimmed.is_empty()
            || !trimmed
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err(PushDlqMirrorDurableError::Invalid);
        }
        Ok(Self(trimmed.to_owned()))
    }

    #[must_use]
    pub fn default_durable() -> Self {
        Self(Self::DEFAULT.to_owned())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushDlqMirrorSettings {
    pub enabled: bool,
    pub durable: PushDlqMirrorDurable,
}

pub fn push_dlq_mirror_settings<E: ReadEnv>(env: &E) -> PushDlqMirrorSettings {
    let enabled = env
        .var(ENV_PUSH_DLQ_MIRROR)
        .ok()
        .is_some_and(|flag| matches!(flag.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"));

    let durable = env
        .var(ENV_PUSH_DLQ_MIRROR_DURABLE)
        .ok()
        .and_then(|raw| PushDlqMirrorDurable::new(raw).ok())
        .unwrap_or_else(PushDlqMirrorDurable::default_durable);

    PushDlqMirrorSettings { enabled, durable }
}

pub fn push_dlq_mirror_pull_config(prefix: &A2aPrefix, durable: &PushDlqMirrorDurable) -> Config {
    Config {
        durable_name: Some(durable.as_str().to_owned()),
        filter_subject: format!("{}.push.dlq.>", prefix.as_str()),
        deliver_policy: DeliverPolicy::All,
        ack_policy: AckPolicy::Explicit,
        replay_policy: ReplayPolicy::Instant,
        ..Default::default()
    }
}

pub fn should_skip_push_dlq_mirror(source_subject: &str, headers: Option<&HeaderMap>) -> bool {
    if source_subject.contains(".mirror.") {
        return true;
    }
    headers.is_some_and(|hdrs| hdrs.get(PUSH_DLQ_MIRROR_HEADER).is_some())
}

pub fn push_dlq_mirror_subject(prefix: &A2aPrefix, source_subject: &str) -> Option<String> {
    if should_skip_push_dlq_mirror(source_subject, None) {
        return None;
    }

    let head = format!("{}.push.dlq.", prefix.as_str());
    let rest = source_subject.strip_prefix(&head)?;
    let mut segments = rest.split('.');
    let caller_id = segments.next()?;
    let task_id = segments.next()?;
    if segments.next().is_some() || caller_id == "mirror" {
        return None;
    }

    Some(format!("{}.push.dlq.mirror.{}.{}", prefix.as_str(), caller_id, task_id))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MirrorDispatchOutcome {
    Skipped,
    Mirrored,
    PublishFailed,
}

pub async fn mirror_push_dlq_envelope<J>(
    js: &J,
    prefix: &A2aPrefix,
    source_subject: &str,
    headers: Option<&HeaderMap>,
    payload: &[u8],
    dedup: &PushDlqDedupGate,
) -> MirrorDispatchOutcome
where
    J: JetStreamPublisher + Clone + Send + Sync,
{
    if should_skip_push_dlq_mirror(source_subject, headers) {
        return MirrorDispatchOutcome::Skipped;
    }

    let Some(mirror_subject) = push_dlq_mirror_subject(prefix, source_subject) else {
        warn!(
            source_subject,
            "push DLQ mirror: source subject does not match expected agent DLQ shape; skipping"
        );
        return MirrorDispatchOutcome::Skipped;
    };

    let Some(idempotency_key) = idempotency_key_from_dlq_payload(payload) else {
        warn!(
            source_subject,
            "push DLQ mirror: envelope missing idempotency_key; skipping mirror publish"
        );
        return MirrorDispatchOutcome::Skipped;
    };

    if !dedup.try_acquire(&idempotency_key) {
        info!(
            source_subject,
            idempotency_key = %idempotency_key,
            "push DLQ mirror publish suppressed by in-process dedup gate"
        );
        return MirrorDispatchOutcome::Skipped;
    }

    let mut mirror_headers = HeaderMap::new();
    mirror_headers.insert(PUSH_DLQ_MIRROR_HEADER, "true");
    mirror_headers.insert(NATS_MSG_ID_HEADER, idempotency_key.as_str());
    if let Some(hdrs) = headers {
        if let Some(content_type) = hdrs.get("Content-Type") {
            mirror_headers.insert("Content-Type", content_type.clone());
        }
    } else {
        mirror_headers.insert("Content-Type", "application/json");
    }

    let payload = Bytes::copy_from_slice(payload);
    for attempt in 1..=MAX_PUBLISH_RETRIES {
        match js
            .publish_with_headers(
                async_nats::Subject::from(mirror_subject.as_str()),
                mirror_headers.clone(),
                payload.clone(),
            )
            .await
        {
            Ok(ack_fut) => match ack_fut.await {
                Ok(_) => {
                    info!(
                        source_subject,
                        mirror_subject = mirror_subject.as_str(),
                        "push DLQ mirror publish acknowledged"
                    );
                    return MirrorDispatchOutcome::Mirrored;
                }
                Err(error) => {
                    warn!(
                        source_subject,
                        mirror_subject = mirror_subject.as_str(),
                        attempt,
                        error = %error,
                        "push DLQ mirror JetStream ack failed"
                    );
                }
            },
            Err(error) => {
                warn!(
                    source_subject,
                    mirror_subject = mirror_subject.as_str(),
                    attempt,
                    error = %error,
                    "push DLQ mirror publish failed"
                );
            }
        }

        if attempt < MAX_PUBLISH_RETRIES {
            tokio::time::sleep(RETRY_BASE_DELAY * attempt).await;
        }
    }

    MirrorDispatchOutcome::PublishFailed
}

fn idempotency_key_from_dlq_payload(payload: &[u8]) -> Option<PushIdempotencyKey> {
    let value: serde_json::Value = serde_json::from_slice(payload).ok()?;
    let key = value.get("idempotency_key")?.as_str()?;
    if key.is_empty() {
        return None;
    }
    Some(PushIdempotencyKey::from_dedupe_wire(key))
}

/// Background pull-consumer that mirrors DLQ envelopes until shutdown.
///
/// Gated behind `cfg(not(coverage))` because it binds a real JetStream
/// context and would block coverage measurement; the pure helpers
/// (`mirror_push_dlq_envelope`, `push_dlq_mirror_subject`, etc.) are
/// exercised by unit tests under all build modes.
#[cfg(not(coverage))]
pub async fn run_push_dlq_mirror(
    js: async_nats::jetstream::Context,
    prefix: A2aPrefix,
    durable: PushDlqMirrorDurable,
    shutdown: CancellationToken,
    dedup: Arc<PushDlqDedupGate>,
) {
    let stream_name = A2aStream::PushDlq.stream_name(&prefix);
    let stream = match js.get_stream(&stream_name).await {
        Ok(stream) => stream,
        Err(error) => {
            warn!(
                stream = stream_name.as_str(),
                error = %error,
                "push DLQ mirror: failed to bind JetStream stream"
            );
            return;
        }
    };

    let consumer_config = push_dlq_mirror_pull_config(&prefix, &durable);
    let consumer = match stream.get_or_create_consumer(durable.as_str(), consumer_config).await {
        Ok(consumer) => consumer,
        Err(error) => {
            warn!(
                stream = stream_name.as_str(),
                durable = durable.as_str(),
                error = %error,
                "push DLQ mirror: failed to provision pull consumer"
            );
            return;
        }
    };

    let mut messages = match consumer.messages().await {
        Ok(messages) => messages,
        Err(error) => {
            warn!(
                durable = durable.as_str(),
                error = %error,
                "push DLQ mirror: consumer messages stream unavailable"
            );
            return;
        }
    };

    info!(
        stream = stream_name.as_str(),
        durable = durable.as_str(),
        "push DLQ mirror consumer started"
    );

    loop {
        tokio::select! {
            () = shutdown.cancelled() => {
                info!(durable = durable.as_str(), "push DLQ mirror shutting down");
                break;
            }
            item = messages.next() => {
                match item {
                    None => {
                        warn!(durable = durable.as_str(), "push DLQ mirror consumer stream ended");
                        break;
                    }
                    Some(Err(error)) => {
                        warn!(durable = durable.as_str(), error = %error, "push DLQ mirror consumer error");
                    }
                    Some(Ok(js_msg)) => {
                        let source_subject = js_msg.subject.to_string();
                        let headers = js_msg.headers.as_ref();
                        let payload = js_msg.payload.as_ref();
                        let outcome = mirror_push_dlq_envelope(
                            &js,
                            &prefix,
                            source_subject.as_str(),
                            headers,
                            payload,
                            dedup.as_ref(),
                        )
                        .await;

                        match outcome {
                            MirrorDispatchOutcome::PublishFailed => {
                                if let Err(error) = js_msg.ack_with(async_nats::jetstream::AckKind::Nak(None)).await {
                                    warn!(
                                        source_subject = source_subject.as_str(),
                                        error = %error,
                                        "push DLQ mirror: failed to NAK source message after publish failure"
                                    );
                                }
                            }
                            MirrorDispatchOutcome::Skipped | MirrorDispatchOutcome::Mirrored => {
                                if let Err(error) = js_msg.ack().await {
                                    warn!(
                                        source_subject = source_subject.as_str(),
                                        error = %error,
                                        "push DLQ mirror: failed to ACK source message"
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(coverage)]
pub async fn run_push_dlq_mirror(
    _js: async_nats::jetstream::Context,
    _prefix: A2aPrefix,
    _durable: PushDlqMirrorDurable,
    _shutdown: CancellationToken,
    _dedup: Arc<PushDlqDedupGate>,
) {
}

#[cfg(test)]
mod tests;
