use std::sync::{Arc, Mutex};

use super::*;
use crate::a2a_prefix::A2aPrefix;
use crate::push::dispatch_error::DispatchError;
use crate::push::push_notification_target::PushNotificationTargetError;
use crate::push::terminal_push_task_state::TerminalPushTaskState;
use crate::task_id::A2aTaskId;
use bytes::Bytes;
use trogon_identity_types::SubjectPrincipal;
use trogon_nats::jetstream::JetStreamPublisher;

#[derive(Clone, Default)]
struct RecordingPublisher {
    publishes: Arc<Mutex<Vec<(String, HeaderMap, Bytes)>>>,
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

/// Publisher whose `publish_with_headers` returns an immediate transport error.
#[derive(Clone, Default)]
struct FailingPublisher;

impl JetStreamPublisher for FailingPublisher {
    type PublishError = std::io::Error;
    type AckFuture = std::future::Ready<Result<async_nats::jetstream::publish::PublishAck, Self::PublishError>>;

    async fn publish_with_headers<S: async_nats::subject::ToSubject + Send>(
        &self,
        _subject: S,
        _headers: HeaderMap,
        _payload: Bytes,
    ) -> Result<Self::AckFuture, Self::PublishError> {
        Err(std::io::Error::other("transport down"))
    }
}

/// Publisher whose publish succeeds but whose ack future resolves to an error.
#[derive(Clone, Default)]
struct AckFailingPublisher;

impl JetStreamPublisher for AckFailingPublisher {
    type PublishError = std::io::Error;
    type AckFuture = std::future::Ready<Result<async_nats::jetstream::publish::PublishAck, Self::PublishError>>;

    async fn publish_with_headers<S: async_nats::subject::ToSubject + Send>(
        &self,
        _subject: S,
        _headers: HeaderMap,
        _payload: Bytes,
    ) -> Result<Self::AckFuture, Self::PublishError> {
        Ok(std::future::ready(Err(std::io::Error::other("ack timeout"))))
    }
}

fn dead_letter(task_id: &str) -> PushDlqSubject {
    PushDlqSubject::new(
        &A2aPrefix::new("a2a".to_string()).unwrap(),
        &SubjectPrincipal::from_tokens("person", "alice").unwrap(),
        &A2aTaskId::new(task_id).unwrap(),
    )
}

fn sample_config() -> a2a::types::TaskPushNotificationConfig {
    a2a::types::TaskPushNotificationConfig {
        id: Some("pcfg-1".to_string()),
        url: "https://example.com/webhook".to_string(),
        task_id: String::new(),
        token: None,
        authentication: None,
        tenant: None,
    }
}

#[test]
fn notification_body_json_preserves_objects() {
    let v = notification_body_json(br#"{"a":1}"#);
    assert_eq!(v, serde_json::json!({"a": 1}));
}

#[test]
fn notification_body_json_fallback_to_string_for_non_utfish() {
    let value = notification_body_json(&[0xffu8, 0xfe]);
    assert!(matches!(value, serde_json::Value::String(ref s) if s.contains('\u{fffd}')));
}

#[tokio::test]
async fn publish_lands_on_the_callers_dead_letter_subject_and_carries_no_caller_header() {
    let js = RecordingPublisher::default();
    let dedup = PushDlqDedupGate::with_capacity(32);
    let err = DispatchError::InvalidTarget(PushNotificationTargetError::UnknownScheme { raw: "bad".into() });

    publish_push_delivery_failure(
        &js,
        &dead_letter("task-1"),
        &sample_config(),
        br#"{}"#,
        &err,
        StatusTransitionId::from_terminal(TerminalPushTaskState::Failed),
        &dedup,
    )
    .await;

    let published = js.publishes.lock().unwrap();
    let (subject, headers, _) = published.first().expect("one dead letter");
    assert_eq!(subject, "a2a.v1.push.dlq.person.alice.task-1");
    let mut names: Vec<String> = headers.iter().map(|(name, _)| name.to_string()).collect();
    names.sort();
    assert_eq!(names, ["Content-Type", NATS_MSG_ID_HEADER]);
}

#[tokio::test]
async fn second_publish_with_same_key_is_suppressed_by_lru() {
    let js = RecordingPublisher::default();
    let dedup = PushDlqDedupGate::with_capacity(32);
    let dead_letter = dead_letter("task-1");
    let config = sample_config();
    let transition = StatusTransitionId::from_terminal(TerminalPushTaskState::Failed);
    let err = DispatchError::InvalidTarget(PushNotificationTargetError::UnknownScheme { raw: "bad".into() });

    publish_push_delivery_failure(&js, &dead_letter, &config, br#"{}"#, &err, transition.clone(), &dedup).await;
    publish_push_delivery_failure(&js, &dead_letter, &config, br#"{}"#, &err, transition, &dedup).await;

    assert_eq!(js.publishes.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn different_transition_ids_both_publish() {
    let js = RecordingPublisher::default();
    let dedup = PushDlqDedupGate::with_capacity(32);
    let dead_letter = dead_letter("task-1");
    let config = sample_config();
    let err = DispatchError::InvalidTarget(PushNotificationTargetError::UnknownScheme { raw: "bad".into() });

    publish_push_delivery_failure(
        &js,
        &dead_letter,
        &config,
        br#"{}"#,
        &err,
        StatusTransitionId::from_terminal(TerminalPushTaskState::Failed),
        &dedup,
    )
    .await;
    publish_push_delivery_failure(
        &js,
        &dead_letter,
        &config,
        br#"{}"#,
        &err,
        StatusTransitionId::from_terminal(TerminalPushTaskState::Completed),
        &dedup,
    )
    .await;

    assert_eq!(js.publishes.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn publish_sets_nats_msg_id_header() {
    let js = RecordingPublisher::default();
    let dedup = PushDlqDedupGate::with_capacity(32);
    let err = DispatchError::InvalidTarget(PushNotificationTargetError::UnknownScheme { raw: "bad".into() });

    publish_push_delivery_failure(
        &js,
        &dead_letter("task-1"),
        &sample_config(),
        br#"{}"#,
        &err,
        StatusTransitionId::from_terminal(TerminalPushTaskState::Failed),
        &dedup,
    )
    .await;

    let published = js.publishes.lock().unwrap();
    assert_eq!(published.len(), 1);
    assert_eq!(
        published[0].1.get(NATS_MSG_ID_HEADER).unwrap().as_str(),
        "3:dlq|6:task-1|6:failed|27:https://example.com/webhook"
    );
}

#[tokio::test]
async fn publish_path_swallows_transport_error_without_panicking() {
    let js = FailingPublisher;
    let dedup = PushDlqDedupGate::default();
    let err = DispatchError::InvalidTarget(PushNotificationTargetError::UnknownScheme { raw: "bad".into() });

    publish_push_delivery_failure(
        &js,
        &dead_letter("task-x"),
        &sample_config(),
        br#"{}"#,
        &err,
        StatusTransitionId::from_terminal(TerminalPushTaskState::Failed),
        &dedup,
    )
    .await;
}

#[tokio::test]
async fn publish_path_swallows_ack_failure_without_panicking() {
    let js = AckFailingPublisher;
    let dedup = PushDlqDedupGate::default();
    let err = DispatchError::InvalidTarget(PushNotificationTargetError::UnknownScheme { raw: "bad".into() });

    publish_push_delivery_failure(
        &js,
        &dead_letter("task-y"),
        &sample_config(),
        br#"{}"#,
        &err,
        StatusTransitionId::from_terminal(TerminalPushTaskState::Failed),
        &dedup,
    )
    .await;
}

#[tokio::test]
async fn publish_uses_empty_push_config_id_when_missing() {
    let js = RecordingPublisher::default();
    let dedup = PushDlqDedupGate::with_capacity(32);
    let mut config = sample_config();
    config.id = None;
    let err = DispatchError::InvalidTarget(PushNotificationTargetError::UnknownScheme { raw: "bad".into() });

    publish_push_delivery_failure(
        &js,
        &dead_letter("task-no-config-id"),
        &config,
        br#"{}"#,
        &err,
        StatusTransitionId::from_terminal(TerminalPushTaskState::Failed),
        &dedup,
    )
    .await;

    let payload = &js.publishes.lock().unwrap()[0].2;
    let body: serde_json::Value = serde_json::from_slice(payload).unwrap();
    assert_eq!(body["push_config_id"], "");
}
