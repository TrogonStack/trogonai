//! `PushDispatcher` impl for `jetstream:` push targets.

use std::time::Duration;

use a2a::types::TaskPushNotificationConfig;
use async_nats::HeaderMap;
use async_nats::header::{IntoHeaderValue, NATS_MESSAGE_ID};
use bytes::Bytes;

use crate::constants::HTTP_PUSH_WEBHOOK_MAX_ATTEMPTS;
use crate::push::delivery_semantics::DeliverySemantics;
use crate::push::dispatch_error::{DispatchError, JetStreamPublishDispatchError};
use crate::push::dispatcher::{PushDispatcher, maybe_terminal_push_idempotency_key};
use crate::push::nats_push_subject::NatsPushSubject;
use crate::push::push_idempotency_key::PushIdempotencyKey;
use crate::push::push_notification_target::{PushNotificationTarget, PushNotificationTargetError};
use crate::push::terminal_push_task_state::TerminalPushTaskState;
use crate::task_id::A2aTaskId;

const INITIAL_RETRY_DELAY: Duration = Duration::from_millis(100);
const MAX_RETRY_DELAY: Duration = Duration::from_secs(3);

pub struct JetStreamPublishPushDispatcher<J> {
    js: J,
}

impl<J> JetStreamPublishPushDispatcher<J> {
    pub fn new(js: J) -> Self {
        Self { js }
    }
}

#[async_trait::async_trait]
impl<J> PushDispatcher for JetStreamPublishPushDispatcher<J>
where
    J: trogon_nats::jetstream::JetStreamPublisher + Clone + Send + Sync + 'static,
{
    async fn dispatch(
        &self,
        task_id: &A2aTaskId,
        config: &TaskPushNotificationConfig,
        delivery_semantics: DeliverySemantics,
        terminal_task_state: TerminalPushTaskState,
        payload: &[u8],
    ) -> Result<(), DispatchError> {
        let subject = parse_jetstream_target(&config.url)?;

        let maybe_key = maybe_terminal_push_idempotency_key(config, task_id, &delivery_semantics, terminal_task_state)
            .map_err(DispatchError::Prep)?;

        let max_attempts = usize::try_from(delivery_semantics.jetstream_max_publish_attempts())
            .unwrap_or(HTTP_PUSH_WEBHOOK_MAX_ATTEMPTS as usize);

        let mut delay = INITIAL_RETRY_DELAY;
        for attempt in 0..max_attempts {
            match publish_with_optional_msg_id(&self.js, &subject, payload, maybe_key.as_ref()).await {
                Ok(()) => return Ok(()),
                Err(e) if attempt + 1 < max_attempts => {
                    tokio::time::sleep(delay).await;
                    delay = next_retry_delay(delay);
                    let _ = e;
                }
                Err(e) => return Err(e),
            }
        }

        Err(DispatchError::JetStreamPublish(JetStreamPublishDispatchError::new(
            subject,
            std::io::Error::other("jetstream publish retry loop exited without resolution"),
        )))
    }
}

fn parse_jetstream_target(raw: &str) -> Result<NatsPushSubject, DispatchError> {
    let PushNotificationTarget::JetStream(subject) = PushNotificationTarget::parse(raw)? else {
        return Err(DispatchError::InvalidTarget(
            PushNotificationTargetError::UnknownScheme { raw: raw.to_owned() },
        ));
    };
    Ok(subject)
}

fn next_retry_delay(current: Duration) -> Duration {
    current.saturating_mul(2).min(MAX_RETRY_DELAY)
}

async fn publish_with_optional_msg_id<J>(
    js: &J,
    subject: &NatsPushSubject,
    payload: &[u8],
    idempotency_key: Option<&PushIdempotencyKey>,
) -> Result<(), DispatchError>
where
    J: trogon_nats::jetstream::JetStreamPublisher + Clone + Send + Sync + 'static,
{
    let mut headers = HeaderMap::new();
    headers.insert("Content-Type", "application/json");
    if let Some(key) = idempotency_key {
        headers.insert(NATS_MESSAGE_ID, key.as_str().into_header_value());
    }

    let ack = js
        .publish_with_headers(
            async_nats::Subject::from(subject.as_str()),
            headers,
            Bytes::copy_from_slice(payload),
        )
        .await
        .map_err(|e| DispatchError::JetStreamPublish(JetStreamPublishDispatchError::new(subject.clone(), e)))?;

    let ack_result = ack
        .await
        .map_err(|e| DispatchError::JetStreamPublish(JetStreamPublishDispatchError::new(subject.clone(), e)))?;

    if ack_result.duplicate {
        tracing::trace!(
            subject = subject.as_str(),
            "JetStream accepted duplicate Msg-Id terminal push ack"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use trogon_nats::jetstream::JetStreamPublisher;

    /// Captures (subject, headers, payload) and resolves the ack future with a
    /// canned PublishAck. `duplicate` controls whether the ack reports the
    /// publish as a Msg-Id collision.
    #[derive(Clone)]
    struct RecordingJetStream {
        captured: Arc<Mutex<Vec<(String, HeaderMap, Bytes)>>>,
        duplicate: bool,
        fail_publish: bool,
        fail_ack: bool,
    }

    impl RecordingJetStream {
        fn new() -> Self {
            Self {
                captured: Arc::default(),
                duplicate: false,
                fail_publish: false,
                fail_ack: false,
            }
        }

        fn with_duplicate(mut self) -> Self {
            self.duplicate = true;
            self
        }

        fn with_publish_failure() -> Self {
            Self {
                captured: Arc::default(),
                duplicate: false,
                fail_publish: true,
                fail_ack: false,
            }
        }

        fn with_ack_failure() -> Self {
            Self {
                captured: Arc::default(),
                duplicate: false,
                fail_publish: false,
                fail_ack: true,
            }
        }
    }

    impl JetStreamPublisher for RecordingJetStream {
        type PublishError = std::io::Error;
        type AckFuture = std::future::Ready<Result<async_nats::jetstream::publish::PublishAck, Self::PublishError>>;

        async fn publish_with_headers<S: async_nats::subject::ToSubject + Send>(
            &self,
            subject: S,
            headers: HeaderMap,
            payload: Bytes,
        ) -> Result<Self::AckFuture, Self::PublishError> {
            if self.fail_publish {
                return Err(std::io::Error::other("publish down"));
            }
            self.captured
                .lock()
                .unwrap()
                .push((subject.to_subject().to_string(), headers, payload));
            if self.fail_ack {
                return Ok(std::future::ready(Err(std::io::Error::other("ack timeout"))));
            }
            Ok(std::future::ready(Ok(async_nats::jetstream::publish::PublishAck {
                duplicate: self.duplicate,
                stream: "A2A_PUSH_DLQ".into(),
                sequence: 1,
                domain: String::new(),
                value: None,
            })))
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
    async fn dispatch_publishes_to_resolved_jetstream_subject() {
        let js = RecordingJetStream::new();
        let dispatcher = JetStreamPublishPushDispatcher::new(js.clone());
        dispatcher
            .dispatch(
                &task(),
                &config("jetstream:a2a.push.bot.caller.t1", Some("cfg-1")),
                DeliverySemantics::AtLeastOnce,
                TerminalPushTaskState::Completed,
                br#"{"ok":true}"#,
            )
            .await
            .unwrap();
        let captured = js.captured.lock().unwrap();
        assert_eq!(captured[0].0, "a2a.push.bot.caller.t1");
    }

    #[tokio::test]
    async fn dispatch_stamps_nats_msg_id_under_exactly_once_semantics() {
        let js = RecordingJetStream::new();
        let dispatcher = JetStreamPublishPushDispatcher::new(js.clone());
        dispatcher
            .dispatch(
                &task(),
                &config("jetstream:a2a.push.bot.caller.t1", Some("cfg-1")),
                DeliverySemantics::ExactlyOnce {
                    idempotency_key_header: None,
                },
                TerminalPushTaskState::Completed,
                br#"{}"#,
            )
            .await
            .unwrap();
        let captured = js.captured.lock().unwrap();
        assert!(captured[0].1.get(NATS_MESSAGE_ID).is_some());
    }

    #[tokio::test]
    async fn dispatch_succeeds_when_jetstream_reports_duplicate_ack() {
        // Duplicate ack just means JetStream already accepted the same
        // Msg-Id — the dispatcher must treat it as success (the desired
        // dedup behaviour, not an error).
        let js = RecordingJetStream::new().with_duplicate();
        let dispatcher = JetStreamPublishPushDispatcher::new(js);
        dispatcher
            .dispatch(
                &task(),
                &config("jetstream:a2a.push.bot.caller.t1", Some("cfg-1")),
                DeliverySemantics::ExactlyOnce {
                    idempotency_key_header: None,
                },
                TerminalPushTaskState::Completed,
                br#"{}"#,
            )
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn dispatch_rejects_non_jetstream_target() {
        let js = RecordingJetStream::new();
        let dispatcher = JetStreamPublishPushDispatcher::new(js);
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
        assert!(matches!(err, DispatchError::InvalidTarget(_)));
    }

    #[tokio::test]
    async fn dispatch_returns_prep_error_when_exactly_once_lacks_config_id() {
        let js = RecordingJetStream::new();
        let dispatcher = JetStreamPublishPushDispatcher::new(js);
        let err = dispatcher
            .dispatch(
                &task(),
                &config("jetstream:a2a.push.bot.caller.t1", None),
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
    async fn dispatch_surfaces_publish_error_after_budget_exhaustion() {
        let js = RecordingJetStream::with_publish_failure();
        let dispatcher = JetStreamPublishPushDispatcher::new(js);
        let err = dispatcher
            .dispatch(
                &task(),
                &config("jetstream:a2a.push.bot.caller.t1", Some("cfg-1")),
                DeliverySemantics::AtLeastOnce,
                TerminalPushTaskState::Completed,
                br#"{}"#,
            )
            .await
            .unwrap_err();
        assert!(matches!(err, DispatchError::JetStreamPublish(_)));
    }

    #[tokio::test]
    async fn dispatch_surfaces_ack_failure_after_budget_exhaustion() {
        let js = RecordingJetStream::with_ack_failure();
        let dispatcher = JetStreamPublishPushDispatcher::new(js);
        let err = dispatcher
            .dispatch(
                &task(),
                &config("jetstream:a2a.push.bot.caller.t1", Some("cfg-1")),
                DeliverySemantics::AtLeastOnce,
                TerminalPushTaskState::Completed,
                br#"{}"#,
            )
            .await
            .unwrap_err();
        assert!(matches!(err, DispatchError::JetStreamPublish(_)));
    }

    #[test]
    fn next_retry_delay_doubles_until_ceiling() {
        assert_eq!(next_retry_delay(Duration::from_millis(100)), Duration::from_millis(200));
        assert_eq!(next_retry_delay(MAX_RETRY_DELAY), MAX_RETRY_DELAY);
    }
}
