//! JetStream publishes for [`super::dispatcher::PushDispatcher`] terminal failures (`A2A_PUSH_DLQ`).

use async_nats::HeaderMap;
use bytes::Bytes;
use serde::Serialize;

use crate::a2a_prefix::A2aPrefix;
use crate::push::CallerId;
use crate::push::caller_id::sanitize_subject_token;
use crate::push::dispatcher::DispatchError;
use crate::push::push_idempotency_key::PushIdempotencyKey;
use crate::task_id::A2aTaskId;

pub(crate) const PUSH_DLQ_SCHEMA_V1: &str = "a2a.push.dlq/v1";

/// `{prefix}.push.dlq.{caller_id}.{task_id}` — trailing tokens match the `A2A_PUSH_DLQ` stream filter `{prefix}.push.dlq.*.*`.
pub(crate) fn push_dlq_publish_subject(prefix: &A2aPrefix, caller_id: &CallerId, task_id: &A2aTaskId) -> String {
    format!(
        "{}.push.dlq.{}.{}",
        prefix.as_str(),
        sanitize_subject_token(caller_id.as_str()),
        task_id.as_str()
    )
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
    caller_id: &CallerId,
    task_id: &A2aTaskId,
    config: &a2a_types::TaskPushNotificationConfig,
    notification_payload: &[u8],
    dispatch_error: &DispatchError,
    idempotency_key: Option<&PushIdempotencyKey>,
) where
    J: trogon_nats::jetstream::JetStreamPublisher + Clone + Send + Sync,
{
    let subject = push_dlq_publish_subject(prefix, caller_id, task_id);

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
    use crate::constants::DEFAULT_PUSH_DLQ_CALLER_SEGMENT;
    use crate::push::resolve_push_dlq_caller_id;

    #[test]
    fn push_dlq_subject_default_caller_round_trips_expected_pattern() {
        let prefix = A2aPrefix::new("a2a".to_string()).unwrap();
        let tid = A2aTaskId::new("task7").unwrap();
        assert_eq!(
            push_dlq_publish_subject(&prefix, &CallerId::default(), &tid),
            "a2a.push.dlq._.task7"
        );
    }

    #[test]
    fn push_dlq_subject_includes_derived_principal_caller_segment() {
        let prefix = A2aPrefix::new("a2a".to_string()).unwrap();
        let tid = A2aTaskId::new("task7").unwrap();
        let cid = CallerId::from_principal(&a2a_auth_callout::SpiceDbPrincipal(
            serde_json::json!({"spicedb_subject": "c1.d2"}),
        ));
        assert_eq!(
            push_dlq_publish_subject(&prefix, &cid, &tid),
            "a2a.push.dlq.c1_d2.task7"
        );
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

    #[test]
    fn resolve_absent_principal_keeps_fallback_caller_segment() {
        let fallback = CallerId::default();
        assert_eq!(
            resolve_push_dlq_caller_id(None, &fallback).as_str(),
            DEFAULT_PUSH_DLQ_CALLER_SEGMENT
        );
    }

    #[test]
    fn resolve_principal_with_spicedb_subject_builds_dlq_subject() {
        let prefix = A2aPrefix::new("a2a".to_string()).unwrap();
        let tid = A2aTaskId::new("task7").unwrap();
        let p = a2a_auth_callout::SpiceDbPrincipal(serde_json::json!({"spicedb_subject": "c1.d2"}));
        let cid = resolve_push_dlq_caller_id(Some(&p), &CallerId::default());
        assert_eq!(push_dlq_publish_subject(&prefix, &cid, &tid), "a2a.push.dlq.c1_d2.task7");
    }

    #[test]
    fn resolve_principal_without_spicedb_subject_falls_back() {
        let prefix = A2aPrefix::new("a2a".to_string()).unwrap();
        let tid = A2aTaskId::new("task7").unwrap();
        let p = a2a_auth_callout::SpiceDbPrincipal(serde_json::json!({}));
        let cid = resolve_push_dlq_caller_id(Some(&p), &CallerId::default());
        assert_eq!(
            push_dlq_publish_subject(&prefix, &cid, &tid),
            "a2a.push.dlq._.task7"
        );
    }
}
