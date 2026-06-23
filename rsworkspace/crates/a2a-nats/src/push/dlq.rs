//! JetStream publishes for push dispatcher terminal failures (`A2A_PUSH_DLQ`).

use async_nats::HeaderMap;
use bytes::Bytes;
use serde::Serialize;

use crate::a2a_prefix::A2aPrefix;
use crate::constants::NATS_MSG_ID_HEADER;
use crate::push::caller_id::{CallerId, sanitize_subject_token};
use crate::push::dispatch_error::DispatchError;
use crate::push::dlq_dedup::PushDlqDedupGate;
use crate::push::push_idempotency_key::PushIdempotencyKey;
use crate::push::status_transition_id::StatusTransitionId;
use crate::task_id::A2aTaskId;

pub const PUSH_DLQ_SCHEMA_V1: &str = "a2a.push.dlq/v1";

/// `{prefix}.push.dlq.{caller_id}.{task_id}` — trailing tokens match the `A2A_PUSH_DLQ` stream filter `{prefix}.push.dlq.*.*`.
pub fn push_dlq_publish_subject(prefix: &A2aPrefix, caller_id: &CallerId, task_id: &A2aTaskId) -> String {
    format!(
        "{}.push.dlq.{}.{}",
        prefix.as_str(),
        sanitize_subject_token(caller_id.as_str()),
        task_id.as_str()
    )
}

/// JSON envelope for terminal push delivery failures (`schema`: **`a2a.push.dlq/v1`**).
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

/// Publishes a JSON failure record onto the push DLQ subject (captures **`A2A_PUSH_DLQ`**).
#[allow(clippy::too_many_arguments)]
pub async fn publish_push_delivery_failure<J>(
    js: &J,
    prefix: &A2aPrefix,
    caller_id: &CallerId,
    task_id: &A2aTaskId,
    config: &a2a::types::TaskPushNotificationConfig,
    notification_payload: &[u8],
    dispatch_error: &DispatchError,
    status_transition_id: StatusTransitionId,
    dedup: &PushDlqDedupGate,
) where
    J: trogon_nats::jetstream::JetStreamPublisher + Clone + Send + Sync,
{
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

    let subject = push_dlq_publish_subject(prefix, caller_id, task_id);

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
        .publish_with_headers(
            async_nats::Subject::from(subject.as_str()),
            headers,
            Bytes::from(bytes_vec),
        )
        .await
    {
        Ok(fut) => {
            if let Err(e) = fut.await {
                tracing::warn!(%task_id, error = %e, "JetStream ack failed for push DLQ publish on {subject}");
            }
        }
        Err(e) => {
            tracing::warn!(%task_id, error = %e, "failed to publish push DLQ message on {subject}");
        }
    }
}

#[cfg(test)]
mod tests;
