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
mod tests;
