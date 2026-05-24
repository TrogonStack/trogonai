use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use a2a_nats::push::{PushDlqDedupGate, PushIdempotencyKey};
use a2a_nats::A2aPrefix;
use a2a_nats::constants::NATS_MSG_ID_HEADER;
use a2a_nats::nats::subjects::A2aStream;
use async_nats::HeaderMap;
use async_nats::jetstream::consumer::pull::Config;
use async_nats::jetstream::consumer::{AckPolicy, DeliverPolicy, ReplayPolicy};
use bytes::Bytes;
use futures::StreamExt;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use trogon_nats::jetstream::JetStreamPublisher;
use trogon_std::env::ReadEnv;

pub const ENV_PUSH_DLQ_MIRROR: &str = "A2A_GATEWAY_PUSH_DLQ_MIRROR";
pub const ENV_PUSH_DLQ_MIRROR_DURABLE: &str = "A2A_GATEWAY_PUSH_DLQ_DURABLE";
pub const PUSH_DLQ_MIRROR_HEADER: &str = "X-A2a-Dlq-Mirrored";

const MAX_PUBLISH_RETRIES: u32 = 3;
const RETRY_BASE_DELAY: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushDlqMirrorDurable(String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushDlqMirrorDurableError;

impl std::fmt::Display for PushDlqMirrorDurableError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("push DLQ mirror durable name must be non-empty ASCII alphanumeric, '-', or '_'")
    }
}

impl std::error::Error for PushDlqMirrorDurableError {}

impl PushDlqMirrorDurable {
    pub const DEFAULT: &'static str = "a2a-gateway-push-dlq-mirror";

    pub fn new(raw: impl AsRef<str>) -> Result<Self, PushDlqMirrorDurableError> {
        let trimmed = raw.as_ref().trim();
        if trimmed.is_empty()
            || !trimmed
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err(PushDlqMirrorDurableError);
        }
        Ok(Self(trimmed.to_owned()))
    }

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

    Some(format!(
        "{}.push.dlq.mirror.{}.{}",
        prefix.as_str(),
        caller_id,
        task_id
    ))
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
    let consumer = match stream
        .get_or_create_consumer(durable.as_str(), consumer_config)
        .await
    {
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

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use a2a_nats::push::PushDlqDedupGate;
    use async_nats::HeaderMap;
    use bytes::Bytes;
    use trogon_nats::jetstream::JetStreamPublisher;
    use trogon_std::env::InMemoryEnv;

    use super::*;

    #[derive(Clone, Default)]
    struct RecordingPublisher {
        publishes: Arc<Mutex<Vec<(String, HeaderMap, Bytes)>>>,
        fail_until: Arc<Mutex<u32>>,
    }

    impl RecordingPublisher {
        fn fail_next_n(&self, n: u32) {
            *self.fail_until.lock().unwrap() = n;
        }
    }

    impl JetStreamPublisher for RecordingPublisher {
        type PublishError = std::io::Error;
        type AckFuture = std::future::Ready<Result<async_nats::jetstream::publish::PublishAck, Self::PublishError>>;

        async fn publish_with_headers<S: async_nats::subject::ToSubject + Send>(
            &self,
            subject: S,
            headers: HeaderMap,
            payload: Bytes,
        ) -> Result<Self::AckFuture, Self::PublishError> {
            let mut remaining = self.fail_until.lock().unwrap();
            if *remaining > 0 {
                *remaining -= 1;
                return Err(std::io::Error::other("simulated publish failure"));
            }
            self.publishes
                .lock()
                .unwrap()
                .push((subject.to_subject().to_string(), headers, payload));
            Ok(std::future::ready(Ok(async_nats::jetstream::publish::PublishAck {
                duplicate: false,
                stream: "A2A_PUSH_DLQ".into(),
                sequence: 1,
                domain: String::new(),
                value: None,
            })))
        }
    }

    fn prefix() -> A2aPrefix {
        A2aPrefix::new("a2a".to_string()).unwrap()
    }

    #[test]
    fn durable_rejects_empty_and_invalid_tokens() {
        assert!(PushDlqMirrorDurable::new("").is_err());
        assert!(PushDlqMirrorDurable::new("bad name").is_err());
        assert_eq!(
            PushDlqMirrorDurable::new("custom-mirror").unwrap().as_str(),
            "custom-mirror"
        );
    }

    #[test]
    fn settings_default_off_without_env() {
        let env = InMemoryEnv::new();
        let settings = push_dlq_mirror_settings(&env);
        assert!(!settings.enabled);
        assert_eq!(settings.durable, PushDlqMirrorDurable::default_durable());
    }

    #[test]
    fn settings_enable_and_durable_override() {
        let env = InMemoryEnv::new();
        env.set(ENV_PUSH_DLQ_MIRROR, "on");
        env.set(ENV_PUSH_DLQ_MIRROR_DURABLE, "ops-mirror");
        let settings = push_dlq_mirror_settings(&env);
        assert!(settings.enabled);
        assert_eq!(settings.durable.as_str(), "ops-mirror");
    }

    #[test]
    fn mirror_subject_maps_agent_dlq_shape() {
        assert_eq!(
            push_dlq_mirror_subject(&prefix(), "a2a.push.dlq.c1.task-9"),
            Some("a2a.push.dlq.mirror.c1.task-9".to_string())
        );
    }

    #[test]
    fn mirror_subject_rejects_existing_mirror_and_extra_tokens() {
        assert!(push_dlq_mirror_subject(&prefix(), "a2a.push.dlq.mirror.c1.task-9").is_none());
        assert!(push_dlq_mirror_subject(&prefix(), "a2a.push.dlq.c1.task-9.extra").is_none());
    }

    #[test]
    fn skip_mirror_loop_markers() {
        assert!(should_skip_push_dlq_mirror(
            "a2a.push.dlq.mirror.c1.task-9",
            None
        ));
        let mut headers = HeaderMap::new();
        headers.insert(PUSH_DLQ_MIRROR_HEADER, "true");
        assert!(should_skip_push_dlq_mirror("a2a.push.dlq.c1.task-9", Some(&headers)));
    }

    #[test]
    fn pull_consumer_filter_uses_prefix_wildcard() {
        let config = push_dlq_mirror_pull_config(&prefix(), &PushDlqMirrorDurable::default_durable());
        assert_eq!(config.filter_subject, "a2a.push.dlq.>");
        assert_eq!(
            config.durable_name.as_deref(),
            Some(PushDlqMirrorDurable::DEFAULT)
        );
    }

    #[tokio::test]
    async fn mirror_publish_adds_header_and_retries() {
        let js = RecordingPublisher::default();
        js.fail_next_n(2);
        let dedup = PushDlqDedupGate::with_capacity(32);
        let payload = br#"{"schema":"a2a.push.dlq/v1","idempotency_key":"task-1:failed:https://example.com/hook"}"#;
        let outcome = mirror_push_dlq_envelope(
            &js,
            &prefix(),
            "a2a.push.dlq.alice.task-1",
            None,
            payload,
            &dedup,
        )
        .await;
        assert_eq!(outcome, MirrorDispatchOutcome::Mirrored);
        let published = js.publishes.lock().unwrap();
        assert_eq!(published.len(), 1);
        assert_eq!(published[0].0, "a2a.push.dlq.mirror.alice.task-1");
        assert!(published[0].1.get(PUSH_DLQ_MIRROR_HEADER).is_some());
        assert_eq!(
            published[0].1.get(NATS_MSG_ID_HEADER).unwrap().as_str(),
            "task-1:failed:https://example.com/hook"
        );
    }

    #[tokio::test]
    async fn mirror_skips_already_mirrored_subjects() {
        let js = RecordingPublisher::default();
        let dedup = PushDlqDedupGate::with_capacity(32);
        let outcome = mirror_push_dlq_envelope(
            &js,
            &prefix(),
            "a2a.push.dlq.mirror.alice.task-1",
            None,
            br#"{"idempotency_key":"k1"}"#,
            &dedup,
        )
        .await;
        assert_eq!(outcome, MirrorDispatchOutcome::Skipped);
        assert!(js.publishes.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn mirror_second_publish_with_same_key_is_suppressed() {
        let js = RecordingPublisher::default();
        let dedup = PushDlqDedupGate::with_capacity(32);
        let payload = br#"{"schema":"a2a.push.dlq/v1","idempotency_key":"task-1:failed:https://example.com/hook"}"#;

        assert_eq!(
            mirror_push_dlq_envelope(
                &js,
                &prefix(),
                "a2a.push.dlq.alice.task-1",
                None,
                payload,
                &dedup,
            )
            .await,
            MirrorDispatchOutcome::Mirrored
        );
        assert_eq!(
            mirror_push_dlq_envelope(
                &js,
                &prefix(),
                "a2a.push.dlq.alice.task-1",
                None,
                payload,
                &dedup,
            )
            .await,
            MirrorDispatchOutcome::Skipped
        );
        assert_eq!(js.publishes.lock().unwrap().len(), 1);
    }
}
