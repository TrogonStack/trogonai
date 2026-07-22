use super::*;
use crate::push::idempotency_key_header::IdempotencyKeyHeader;
use crate::push::push_idempotency_key::PushIdempotencyKey;

fn config_with_id(id: Option<&str>) -> TaskPushNotificationConfig {
    TaskPushNotificationConfig {
        url: "https://example.com/hook".into(),
        id: id.map(str::to_owned),
        task_id: String::new(),
        token: None,
        authentication: None,
        tenant: None,
    }
}

fn task() -> A2aTaskId {
    A2aTaskId::new("task-1").unwrap()
}

#[test]
fn maybe_terminal_returns_none_for_at_least_once_semantics() {
    let config = config_with_id(Some("cfg-1"));
    let result = maybe_terminal_push_idempotency_key(
        &config,
        &task(),
        &DeliverySemantics::AtLeastOnce,
        TerminalPushTaskState::Completed,
    )
    .unwrap();
    assert!(result.is_none(), "AtLeastOnce must not derive an idempotency key");
}

#[test]
fn maybe_terminal_returns_key_for_exactly_once_semantics() {
    let config = config_with_id(Some("cfg-1"));
    let key = maybe_terminal_push_idempotency_key(
        &config,
        &task(),
        &DeliverySemantics::ExactlyOnce {
            idempotency_key_header: None,
        },
        TerminalPushTaskState::Completed,
    )
    .unwrap()
    .expect("exactly-once must derive a key");
    let expected = PushIdempotencyKey::derive_terminal(
        &task(),
        &PushNotificationConfigId::new("cfg-1").unwrap(),
        TerminalPushTaskState::Completed,
    );
    assert_eq!(key.as_str(), expected.as_str());
}

#[test]
fn maybe_terminal_returns_key_when_config_id_is_missing_via_default() {
    // A missing/None config id defaults to the empty string at the wire
    // boundary, which `PushNotificationConfigId::new` rejects with Empty;
    // the helper must propagate that as DispatchPrepError::PushConfigId.
    let config = config_with_id(None);
    let err = maybe_terminal_push_idempotency_key(
        &config,
        &task(),
        &DeliverySemantics::ExactlyOnce {
            idempotency_key_header: None,
        },
        TerminalPushTaskState::Completed,
    )
    .unwrap_err();
    assert!(matches!(err, DispatchPrepError::PushConfigId(_)));
}

#[test]
fn maybe_terminal_propagates_custom_idempotency_header_under_exactly_once() {
    // The carrier header doesn't affect key derivation, but the helper
    // still emits a key whenever exactly-once is requested with any header.
    let config = config_with_id(Some("cfg-2"));
    let key = maybe_terminal_push_idempotency_key(
        &config,
        &task(),
        &DeliverySemantics::ExactlyOnce {
            idempotency_key_header: Some(IdempotencyKeyHeader::try_from("X-Push-Key").unwrap()),
        },
        TerminalPushTaskState::Failed,
    )
    .unwrap();
    assert!(key.is_some());
}

#[test]
fn webhook_http_retryable_returns_true_for_documented_retry_set() {
    for status in [408u16, 425, 429, 500, 501, 502, 503, 504, 522, 524] {
        assert!(webhook_http_retryable(status), "{status} must be in the retry set");
    }
}

#[test]
fn webhook_http_retryable_returns_false_for_terminal_statuses() {
    for status in [200u16, 201, 301, 400, 401, 403, 404, 410, 418, 426, 451] {
        assert!(!webhook_http_retryable(status), "{status} must not be retried");
    }
}
