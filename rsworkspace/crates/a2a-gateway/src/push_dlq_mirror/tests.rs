use std::sync::Mutex;

use a2a_nats::push::dlq_dedup::PushDlqDedupGate;
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
    assert!(matches!(
        PushDlqMirrorDurable::new(""),
        Err(PushDlqMirrorDurableError::Invalid)
    ));
    assert!(matches!(
        PushDlqMirrorDurable::new("bad name"),
        Err(PushDlqMirrorDurableError::Invalid)
    ));
    assert_eq!(
        PushDlqMirrorDurable::new("custom-mirror").unwrap().as_str(),
        "custom-mirror"
    );
}

#[test]
fn durable_default_matches_documented_constant() {
    assert_eq!(
        PushDlqMirrorDurable::default_durable().as_str(),
        PushDlqMirrorDurable::DEFAULT
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
fn settings_ignore_invalid_durable_env_value() {
    // A bad durable env value falls back to the default rather than failing
    // the whole settings resolution — operators see the default in logs and
    // can fix the env.
    let env = InMemoryEnv::new();
    env.set(ENV_PUSH_DLQ_MIRROR_DURABLE, "spaces not allowed");
    let settings = push_dlq_mirror_settings(&env);
    assert_eq!(settings.durable, PushDlqMirrorDurable::default_durable());
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
fn mirror_subject_rejects_unrelated_subject() {
    assert!(push_dlq_mirror_subject(&prefix(), "different.subject").is_none());
}

#[test]
fn skip_mirror_loop_markers() {
    assert!(should_skip_push_dlq_mirror("a2a.push.dlq.mirror.c1.task-9", None));
    let mut headers = HeaderMap::new();
    headers.insert(PUSH_DLQ_MIRROR_HEADER, "true");
    assert!(should_skip_push_dlq_mirror("a2a.push.dlq.c1.task-9", Some(&headers)));
}

#[test]
fn pull_consumer_filter_uses_prefix_wildcard() {
    let config = push_dlq_mirror_pull_config(&prefix(), &PushDlqMirrorDurable::default_durable());
    assert_eq!(config.filter_subject, "a2a.push.dlq.>");
    assert_eq!(config.durable_name.as_deref(), Some(PushDlqMirrorDurable::DEFAULT));
}

#[tokio::test]
async fn mirror_publish_adds_header_and_retries() {
    let js = RecordingPublisher::default();
    js.fail_next_n(2);
    let dedup = PushDlqDedupGate::with_capacity(32);
    let payload = br#"{"schema":"a2a.push.dlq/v1","idempotency_key":"task-1:failed:https://example.com/hook"}"#;
    let outcome = mirror_push_dlq_envelope(&js, &prefix(), "a2a.push.dlq.alice.task-1", None, payload, &dedup).await;
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
        mirror_push_dlq_envelope(&js, &prefix(), "a2a.push.dlq.alice.task-1", None, payload, &dedup).await,
        MirrorDispatchOutcome::Mirrored
    );
    assert_eq!(
        mirror_push_dlq_envelope(&js, &prefix(), "a2a.push.dlq.alice.task-1", None, payload, &dedup).await,
        MirrorDispatchOutcome::Skipped
    );
    assert_eq!(js.publishes.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn mirror_publish_failed_when_all_attempts_fail() {
    let js = RecordingPublisher::default();
    // Fail more attempts than the retry budget so the loop exhausts.
    js.fail_next_n(MAX_PUBLISH_RETRIES + 1);
    let dedup = PushDlqDedupGate::with_capacity(32);
    let payload = br#"{"idempotency_key":"task-2:failed"}"#;
    let outcome = mirror_push_dlq_envelope(&js, &prefix(), "a2a.push.dlq.bob.task-2", None, payload, &dedup).await;
    assert_eq!(outcome, MirrorDispatchOutcome::PublishFailed);
    assert!(js.publishes.lock().unwrap().is_empty());
}

#[tokio::test]
async fn mirror_publish_failure_releases_dedup_so_redelivery_can_retry() {
    // Regression: try_acquire reserved the key BEFORE publish; if publish
    // failed and the consumer NAKed, the JetStream redelivery would see the
    // key still reserved and short-circuit as Skipped, ACKing without
    // mirroring. The release must run on PublishFailed so a redelivery can
    // retry the publish.
    let js = RecordingPublisher::default();
    js.fail_next_n(MAX_PUBLISH_RETRIES + 1);
    let dedup = PushDlqDedupGate::with_capacity(32);
    let payload = br#"{"idempotency_key":"task-3:failed"}"#;
    assert_eq!(
        mirror_push_dlq_envelope(&js, &prefix(), "a2a.push.dlq.alice.task-3", None, payload, &dedup).await,
        MirrorDispatchOutcome::PublishFailed
    );

    // Second attempt — let publish succeed this time. The redelivery must
    // NOT be dedup-suppressed.
    let outcome = mirror_push_dlq_envelope(&js, &prefix(), "a2a.push.dlq.alice.task-3", None, payload, &dedup).await;
    assert_eq!(outcome, MirrorDispatchOutcome::Mirrored);
    assert_eq!(js.publishes.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn mirror_skips_payload_without_idempotency_key() {
    let js = RecordingPublisher::default();
    let dedup = PushDlqDedupGate::with_capacity(32);
    let outcome = mirror_push_dlq_envelope(
        &js,
        &prefix(),
        "a2a.push.dlq.alice.task-1",
        None,
        br#"{"schema":"a2a.push.dlq/v1"}"#,
        &dedup,
    )
    .await;
    assert_eq!(outcome, MirrorDispatchOutcome::Skipped);
}

#[tokio::test]
async fn mirror_carries_content_type_header_through() {
    let js = RecordingPublisher::default();
    let dedup = PushDlqDedupGate::with_capacity(32);
    let mut source_headers = HeaderMap::new();
    source_headers.insert("Content-Type", "application/cbor");
    let payload = br#"{"idempotency_key":"k1"}"#;
    let outcome = mirror_push_dlq_envelope(
        &js,
        &prefix(),
        "a2a.push.dlq.alice.task-1",
        Some(&source_headers),
        payload,
        &dedup,
    )
    .await;
    assert_eq!(outcome, MirrorDispatchOutcome::Mirrored);
    let published = js.publishes.lock().unwrap();
    assert_eq!(published[0].1.get("Content-Type").unwrap().as_str(), "application/cbor");
}
