//! JetStream publishes for [`super::dispatcher::PushDispatcher`] terminal failures (`A2A_PUSH_DLQ`).

use std::borrow::Cow;

use async_nats::HeaderMap;
use bytes::Bytes;
use serde::Serialize;

use crate::a2a_prefix::A2aPrefix;
use crate::push::dispatcher::DispatchError;
use crate::push::push_idempotency_key::PushIdempotencyKey;
use crate::task_id::A2aTaskId;

pub(crate) const PUSH_DLQ_SCHEMA_V1: &str = "a2a.push.dlq/v1";

/// `{prefix}.push.dlq.{caller_id}.{task_id}` — trailing tokens match the `A2A_PUSH_DLQ` stream filter `{prefix}.push.dlq.*.*`.
pub(crate) fn push_dlq_publish_subject(prefix: &A2aPrefix, caller_segment: &str, task_id: &A2aTaskId) -> String {
    format!(
        "{}.push.dlq.{}.{}",
        prefix.as_str(),
        sanitize_subject_token(caller_segment),
        task_id.as_str()
    )
}

fn sanitize_subject_token(raw: &str) -> Cow<'_, str> {
    // NATS `.` breaks the `{caller_id}` single-token invariant for this segment.
    const fn forbidden(c: char) -> bool {
        matches!(c, '.' | '*' | '>' | ' ' | '\t' | '\n' | '\r' | '\0'..='\x1f')
    }

    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Cow::Borrowed("_");
    }

    if !trimmed.chars().any(forbidden) {
        Cow::Borrowed(trimmed)
    } else {
        Cow::Owned(trimmed.chars().map(|c| if forbidden(c) { '_' } else { c }).collect())
    }
}

#[derive(Serialize)]
pub(crate) struct PushDlqMessageV1<'a> {
    pub schema: &'static str,
    pub task_id: &'a str,
    pub push_config_id: &'a str,
    pub target_url: &'a str,
    pub error: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<&'a str>,
    pub notification: serde_json::Value,
}

fn notification_body_json(payload: &[u8]) -> serde_json::Value {
    serde_json::from_slice::<serde_json::Value>(payload)
        .unwrap_or_else(|_| serde_json::Value::String(String::from_utf8_lossy(payload).into_owned()))
}

/// Publishes a JSON failure record onto the push DLQ subject (captures **`A2A_PUSH_DLQ`**).
#[allow(clippy::too_many_arguments)]
pub(crate) async fn publish_push_delivery_failure<J>(
    js: &J,
    prefix: &A2aPrefix,
    caller_segment: &str,
    task_id: &A2aTaskId,
    config: &a2a_types::TaskPushNotificationConfig,
    notification_payload: &[u8],
    dispatch_error: &DispatchError,
    idempotency_key: Option<&PushIdempotencyKey>,
) where
    J: trogon_nats::jetstream::JetStreamPublisher + Clone + Send + Sync,
{
    let subject = push_dlq_publish_subject(prefix, caller_segment, task_id);

    let body = PushDlqMessageV1 {
        schema: PUSH_DLQ_SCHEMA_V1,
        task_id: task_id.as_str(),
        push_config_id: config.id.as_str(),
        target_url: config.url.as_str(),
        error: dispatch_error.to_string(),
        idempotency_key: idempotency_key.map(PushIdempotencyKey::as_str),
        notification: notification_body_json(notification_payload),
    };

    let Ok(bytes_vec) = serde_json::to_vec(&body) else {
        tracing::warn!(
            subject = subject.as_str(),
            task_id = %task_id,
            push_config_id = %config.id,
            "failed to serialize push DLQ envelope; skipping publish"
        );
        return;
    };

    let mut headers = HeaderMap::new();
    headers.insert("Content-Type", "application/json");

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
                tracing::warn!(
                    subject = subject.as_str(),
                    task_id = %task_id,
                    error = %e,
                    "JetStream ack failed for push DLQ publish"
                );
            }
        }
        Err(e) => tracing::warn!(
            subject = subject.as_str(),
            task_id = %task_id,
            error = %e,
            "failed to publish push DLQ message"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::a2a_prefix::A2aPrefix;

    #[test]
    fn push_dlq_subject_default_caller_round_trips_expected_pattern() {
        let prefix = A2aPrefix::new("a2a".to_string()).unwrap();
        let tid = A2aTaskId::new("task7").unwrap();
        assert_eq!(push_dlq_publish_subject(&prefix, "_", &tid), "a2a.push.dlq._.task7");
    }

    #[test]
    fn sanitize_replaces_spaces_and_dots() {
        assert_eq!(sanitize_subject_token(" u1.id ").as_ref(), "u1_id",);
        assert_eq!(sanitize_subject_token("").as_ref(), "_");
    }

    #[test]
    fn notification_body_json_preserves_objects() {
        let v = notification_body_json(br#"{"a":1}"#);
        assert_eq!(v, serde_json::json!({"a": 1}));
    }

    #[test]
    fn notification_body_json_fallback_to_string_for_non_utfish() {
        let payload = &[0xffu8, 0xfe];
        match notification_body_json(payload) {
            serde_json::Value::String(s) => assert!(s.contains('\u{fffd}')),
            _ => panic!("expected string fallback"),
        }
    }
}
