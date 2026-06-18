//! `PushDispatcher` impl for `subject:` push targets (core NATS publish).

use std::time::Duration;

use a2a::types::TaskPushNotificationConfig;
use async_nats::HeaderMap;
use async_nats::header::{IntoHeaderValue, NATS_MESSAGE_ID};
use bytes::Bytes;

use crate::constants::HTTP_PUSH_WEBHOOK_MAX_ATTEMPTS;
use crate::push::delivery_semantics::DeliverySemantics;
use crate::push::dispatch_error::{DispatchError, NatsPublishDispatchError};
use crate::push::dispatcher::{PushDispatcher, maybe_terminal_push_idempotency_key};
use crate::push::nats_push_subject::NatsPushSubject;
use crate::push::push_idempotency_key::PushIdempotencyKey;
use crate::push::push_notification_target::{PushNotificationTarget, PushNotificationTargetError};
use crate::push::terminal_push_task_state::TerminalPushTaskState;
use crate::task_id::A2aTaskId;

const INITIAL_RETRY_DELAY: Duration = Duration::from_millis(100);
const MAX_RETRY_DELAY: Duration = Duration::from_secs(3);

pub struct NatsPublishPushDispatcher<N> {
    nats: N,
}

impl<N> NatsPublishPushDispatcher<N> {
    pub fn new(nats: N) -> Self {
        Self { nats }
    }
}

#[async_trait::async_trait]
impl<N> PushDispatcher for NatsPublishPushDispatcher<N>
where
    N: trogon_nats::PublishClient + Clone + Send + Sync + 'static,
{
    async fn dispatch(
        &self,
        task_id: &A2aTaskId,
        config: &TaskPushNotificationConfig,
        delivery_semantics: DeliverySemantics,
        terminal_task_state: TerminalPushTaskState,
        payload: &[u8],
    ) -> Result<(), DispatchError> {
        let subject = parse_nats_target(&config.url)?;

        let maybe_key = maybe_terminal_push_idempotency_key(config, task_id, &delivery_semantics, terminal_task_state)
            .map_err(DispatchError::Prep)?;

        let max_attempts = usize::try_from(delivery_semantics.nats_core_publish_max_attempts())
            .unwrap_or(HTTP_PUSH_WEBHOOK_MAX_ATTEMPTS as usize);

        let mut delay = INITIAL_RETRY_DELAY;
        for attempt in 0..max_attempts {
            match publish_with_optional_msg_id(&self.nats, &subject, payload, maybe_key.as_ref()).await {
                Ok(()) => return Ok(()),
                Err(e) if attempt + 1 < max_attempts => {
                    tokio::time::sleep(delay).await;
                    delay = next_retry_delay(delay);
                    let _ = e;
                }
                Err(e) => return Err(e),
            }
        }

        Err(DispatchError::NatsPublish(NatsPublishDispatchError::new(
            subject,
            std::io::Error::other("nats publish retry loop exited without resolution"),
        )))
    }
}

fn parse_nats_target(raw: &str) -> Result<NatsPushSubject, DispatchError> {
    let PushNotificationTarget::Nats(subject) = PushNotificationTarget::parse(raw)? else {
        return Err(DispatchError::InvalidTarget(
            PushNotificationTargetError::UnknownScheme { raw: raw.to_owned() },
        ));
    };
    Ok(subject)
}

fn next_retry_delay(current: Duration) -> Duration {
    current.saturating_mul(2).min(MAX_RETRY_DELAY)
}

async fn publish_with_optional_msg_id<N>(
    nats: &N,
    subject: &NatsPushSubject,
    payload: &[u8],
    idempotency_key: Option<&PushIdempotencyKey>,
) -> Result<(), DispatchError>
where
    N: trogon_nats::PublishClient + Clone + Send + Sync + 'static,
{
    let mut headers = HeaderMap::new();
    headers.insert("Content-Type", "application/json");
    if let Some(key) = idempotency_key {
        headers.insert(NATS_MESSAGE_ID, key.as_str().into_header_value());
    }
    nats.publish_with_headers(
        async_nats::Subject::from(subject.as_str()),
        headers,
        Bytes::copy_from_slice(payload),
    )
    .await
    .map_err(|e| DispatchError::NatsPublish(NatsPublishDispatchError::new(subject.clone(), e)))
}

#[cfg(test)]
mod tests {
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
}
