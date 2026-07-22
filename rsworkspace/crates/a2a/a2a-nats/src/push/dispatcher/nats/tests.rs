use super::*;
use std::sync::{Arc, Mutex};
use trogon_nats::{AdvancedMockNatsClient, PublishClient};

/// Capturing test publisher — needed because the workspace mock drops
/// headers on the floor, but the dispatcher's exactly-once contract is
/// "header set iff key required" and we assert on both.
#[derive(Clone, Default)]
struct RecordingPublisher {
    captured: Arc<Mutex<Vec<(String, HeaderMap, Bytes)>>>,
}

impl PublishClient for RecordingPublisher {
    type PublishError = std::io::Error;

    async fn publish_with_headers<S: async_nats::subject::ToSubject + Send>(
        &self,
        subject: S,
        headers: HeaderMap,
        payload: Bytes,
    ) -> Result<(), Self::PublishError> {
        self.captured
            .lock()
            .unwrap()
            .push((subject.to_subject().to_string(), headers, payload));
        Ok(())
    }
}

fn task() -> A2aTaskId {
    A2aTaskId::new("task-1").unwrap()
}

fn config(url: &str, id: Option<&str>) -> TaskPushNotificationConfig {
    TaskPushNotificationConfig {
        url: url.into(),
        id: id.map(str::to_owned),
        task_id: String::new(),
        token: None,
        authentication: None,
        tenant: None,
    }
}

#[tokio::test]
async fn dispatch_publishes_to_resolved_subject() {
    let nats = AdvancedMockNatsClient::new();
    let dispatcher = NatsPublishPushDispatcher::new(nats.clone());

    dispatcher
        .dispatch(
            &task(),
            &config("subject:a2a.push.bot.caller.t1", Some("cfg-1")),
            DeliverySemantics::AtLeastOnce,
            TerminalPushTaskState::Completed,
            br#"{"ok":true}"#,
        )
        .await
        .unwrap();

    let published = nats.published_messages();
    assert_eq!(published, vec!["a2a.push.bot.caller.t1"]);
}

#[tokio::test]
async fn dispatch_stamps_nats_msg_id_under_exactly_once_semantics() {
    let nats = RecordingPublisher::default();
    let dispatcher = NatsPublishPushDispatcher::new(nats.clone());

    dispatcher
        .dispatch(
            &task(),
            &config("subject:a2a.push.bot.caller.t1", Some("cfg-1")),
            DeliverySemantics::ExactlyOnce {
                idempotency_key_header: None,
            },
            TerminalPushTaskState::Completed,
            br#"{"ok":true}"#,
        )
        .await
        .unwrap();

    let captured = nats.captured.lock().unwrap();
    let (_subject, headers, _payload) = &captured[0];
    let value = headers.get(NATS_MESSAGE_ID).expect("Nats-Msg-Id header set");
    assert!(value.as_str().contains("terminal"));
}

#[tokio::test]
async fn dispatch_omits_nats_msg_id_under_at_least_once_semantics() {
    let nats = RecordingPublisher::default();
    let dispatcher = NatsPublishPushDispatcher::new(nats.clone());

    dispatcher
        .dispatch(
            &task(),
            &config("subject:a2a.push.bot.caller.t1", Some("cfg-1")),
            DeliverySemantics::AtLeastOnce,
            TerminalPushTaskState::Completed,
            br#"{"ok":true}"#,
        )
        .await
        .unwrap();

    let captured = nats.captured.lock().unwrap();
    let (_subject, headers, _payload) = &captured[0];
    assert!(headers.get(NATS_MESSAGE_ID).is_none());
}

#[tokio::test]
async fn dispatch_rejects_non_nats_target() {
    let nats = AdvancedMockNatsClient::new();
    let dispatcher = NatsPublishPushDispatcher::new(nats);
    let err = dispatcher
        .dispatch(
            &task(),
            &config("https://example.com/hook", Some("cfg-1")),
            DeliverySemantics::AtLeastOnce,
            TerminalPushTaskState::Completed,
            br#"{}"#,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, DispatchError::InvalidTarget(_)));
}

#[tokio::test]
async fn dispatch_returns_prep_error_when_exactly_once_lacks_config_id() {
    let nats = AdvancedMockNatsClient::new();
    let dispatcher = NatsPublishPushDispatcher::new(nats);
    let err = dispatcher
        .dispatch(
            &task(),
            &config("subject:a2a.push.bot.caller.t1", None),
            DeliverySemantics::ExactlyOnce {
                idempotency_key_header: None,
            },
            TerminalPushTaskState::Completed,
            br#"{}"#,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, DispatchError::Prep(_)));
}

#[tokio::test]
async fn dispatch_retries_then_succeeds_under_at_least_once() {
    let nats = AdvancedMockNatsClient::new();
    nats.fail_publish_count(1);
    let dispatcher = NatsPublishPushDispatcher::new(nats.clone());
    dispatcher
        .dispatch(
            &task(),
            &config("subject:a2a.push.bot.caller.t1", Some("cfg-1")),
            DeliverySemantics::AtLeastOnce,
            TerminalPushTaskState::Completed,
            br#"{"ok":true}"#,
        )
        .await
        .unwrap();
    assert_eq!(nats.published_messages().len(), 1, "second attempt landed");
}

#[tokio::test]
async fn dispatch_surfaces_publish_error_after_budget_exhaustion() {
    let nats = AdvancedMockNatsClient::new();
    // Under AtLeastOnce the budget is HTTP_PUSH_WEBHOOK_MAX_ATTEMPTS (3);
    // failing every attempt must surface NatsPublish.
    nats.fail_publish_count(10);
    let dispatcher = NatsPublishPushDispatcher::new(nats.clone());
    let err = dispatcher
        .dispatch(
            &task(),
            &config("subject:a2a.push.bot.caller.t1", Some("cfg-1")),
            DeliverySemantics::AtLeastOnce,
            TerminalPushTaskState::Completed,
            br#"{}"#,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, DispatchError::NatsPublish(_)));
}

#[test]
fn next_retry_delay_doubles_until_ceiling() {
    assert_eq!(next_retry_delay(Duration::from_millis(100)), Duration::from_millis(200));
    assert_eq!(next_retry_delay(MAX_RETRY_DELAY), MAX_RETRY_DELAY);
}
