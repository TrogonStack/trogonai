//! JetStream publishes for push dispatcher terminal failures (`A2A_PUSH_DLQ`).

use async_nats::HeaderMap;
use async_nats::subject::ToSubject;
use bytes::Bytes;
use serde::Serialize;

use crate::constants::NATS_MSG_ID_HEADER;
pub use crate::constants::PUSH_DLQ_SCHEMA_V1;
use crate::nats::subjects::push::PushDlqSubject;
use crate::push::dispatch_error::DispatchError;
use crate::push::dlq_dedup::PushDlqDedupGate;
use crate::push::push_idempotency_key::PushIdempotencyKey;
use crate::push::status_transition_id::StatusTransitionId;

/// JSON envelope for terminal push delivery failures (`schema`: **`a2a.v1.push.dlq/v1`**).
///
/// `idempotency_key` is deterministic: `{task_id}:{status_transition_id}:{target_url}`.
#[derive(Serialize)]
pub struct PushDlqMessageV1<'a> {
    pub schema: &'static str,
    pub task_id: &'a str,
    pub push_config_id: &'a str,
    pub target_url: &'a str,
    pub error: String,
    pub idempotency_key: &'a str,
    pub notification: serde_json::Value,
}

fn notification_body_json(payload: &[u8]) -> serde_json::Value {
    serde_json::from_slice::<serde_json::Value>(payload)
        .unwrap_or_else(|_| serde_json::Value::String(String::from_utf8_lossy(payload).into_owned()))
}

/// Publishes a JSON failure record onto the caller's dead-letter subject (captured by **`A2A_PUSH_DLQ`**).
pub async fn publish_push_delivery_failure<J>(
    js: &J,
    dead_letter: &PushDlqSubject,
    config: &a2a::types::TaskPushNotificationConfig,
    notification_payload: &[u8],
    dispatch_error: &DispatchError,
    status_transition_id: StatusTransitionId,
    dedup: &PushDlqDedupGate,
) where
    J: trogon_nats::jetstream::JetStreamPublisher + Clone + Send + Sync,
{
    let task_id = dead_letter.task_id();
    let idempotency_key = PushIdempotencyKey::derive_dlq(task_id, &status_transition_id, config.url.as_str());

    let config_id_str = config.id.as_deref().unwrap_or("");
    if !dedup.try_acquire(&idempotency_key) {
        tracing::info!(
            task_id = %task_id,
            push_config_id = config_id_str,
            idempotency_key = %idempotency_key,
            "push DLQ publish suppressed by in-process dedup gate"
        );
        return;
    }

    let body = PushDlqMessageV1 {
        schema: PUSH_DLQ_SCHEMA_V1,
        task_id: task_id.as_str(),
        push_config_id: config_id_str,
        target_url: config.url.as_str(),
        error: dispatch_error.to_string(),
        idempotency_key: idempotency_key.as_str(),
        notification: notification_body_json(notification_payload),
    };

    // PushDlqMessageV1 is plain Serialize — serde_json::to_vec never fails at
    // runtime for it, so fall back to an empty body in the impossible error
    // case instead of carrying a dead match arm forever.
    let bytes_vec = serde_json::to_vec(&body).unwrap_or_default();

    let mut headers = HeaderMap::new();
    headers.insert("Content-Type", "application/json");
    headers.insert(NATS_MSG_ID_HEADER, idempotency_key.as_str());

    match js
        .publish_with_headers(dead_letter.to_subject(), headers, Bytes::from(bytes_vec))
        .await
    {
        Ok(fut) => {
            if let Err(e) = fut.await {
                tracing::warn!(%task_id, subject = %dead_letter, error = %e, "JetStream ack failed for push DLQ publish");
            }
        }
        Err(e) => {
            tracing::warn!(%task_id, subject = %dead_letter, error = %e, "failed to publish push DLQ message");
        }
    }
}

#[cfg(test)]
mod tests;
