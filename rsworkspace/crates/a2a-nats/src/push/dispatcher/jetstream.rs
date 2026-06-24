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
        tracing::trace!("JetStream accepted duplicate Msg-Id terminal push ack on {subject}");
    }
    Ok(())
}

#[cfg(test)]
mod tests;
