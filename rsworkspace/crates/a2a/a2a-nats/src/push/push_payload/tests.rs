use super::*;
use crate::push::push_notification_config_id::PushNotificationConfigId;
use crate::push::terminal_push_task_state::TerminalPushTaskState;
use crate::task_id::A2aTaskId;
use std::error::Error as _;

fn key() -> PushIdempotencyKey {
    PushIdempotencyKey::derive_terminal(
        &A2aTaskId::new("task-1").unwrap(),
        &PushNotificationConfigId::new("cfg-1").unwrap(),
        TerminalPushTaskState::Completed,
    )
}

#[test]
fn pass_through_when_idempotency_not_required_by_semantics() {
    let semantics = DeliverySemantics::default();
    let base = br#"{"hello":"world"}"#;
    let augmented = augment_terminal_push_notification_bytes(base, &semantics, Some(&key())).unwrap();
    assert!(matches!(augmented, Cow::Borrowed(_)));
    assert_eq!(augmented.as_ref(), base);
}

#[test]
fn pass_through_when_at_least_once_and_no_key_supplied() {
    let semantics = DeliverySemantics::AtLeastOnce;
    let base = br#"{"hello":"world"}"#;
    let augmented = augment_terminal_push_notification_bytes(base, &semantics, None).unwrap();
    assert!(matches!(augmented, Cow::Borrowed(_)));
}

#[test]
fn exactly_once_without_key_is_rejected_instead_of_silently_downgraded() {
    let semantics = DeliverySemantics::ExactlyOnce {
        idempotency_key_header: None,
    };
    let base = br#"{"hello":"world"}"#;
    let err = augment_terminal_push_notification_bytes(base, &semantics, None).unwrap_err();
    assert!(matches!(err, PushPayloadAugmentError::MissingIdempotencyKey));
}

#[test]
fn injects_idempotency_field_when_required_and_key_present() {
    let semantics = DeliverySemantics::ExactlyOnce {
        idempotency_key_header: None,
    };
    let base = br#"{"hello":"world"}"#;
    let augmented = augment_terminal_push_notification_bytes(base, &semantics, Some(&key())).unwrap();
    let parsed: Value = serde_json::from_slice(&augmented).unwrap();
    assert_eq!(parsed["hello"], "world");
    assert!(
        parsed[PUSH_IDEMPOTENCY_JSON_FIELD]
            .as_str()
            .unwrap()
            .contains("terminal")
    );
}

#[test]
fn non_object_base_rejected_when_idempotency_key_required() {
    let semantics = DeliverySemantics::ExactlyOnce {
        idempotency_key_header: None,
    };
    for base in [b"[1,2,3]".as_ref(), b"\"hi\"", b"42", b"null"] {
        let err = augment_terminal_push_notification_bytes(base, &semantics, Some(&key())).unwrap_err();
        assert!(matches!(
            err,
            PushPayloadAugmentError::NonObjectPayloadForIdempotencyKey
        ));
    }
}

#[test]
fn invalid_json_base_surfaces_serde_error() {
    let semantics = DeliverySemantics::ExactlyOnce {
        idempotency_key_header: None,
    };
    let base = b"not json";
    let err = augment_terminal_push_notification_bytes(base, &semantics, Some(&key())).unwrap_err();
    assert!(matches!(err, PushPayloadAugmentError::Json(_)));
}

#[test]
fn augment_error_display_and_source_covers_every_variant() {
    let non_object = PushPayloadAugmentError::NonObjectPayloadForIdempotencyKey;
    assert_eq!(
        non_object.to_string(),
        "exactly-once idempotency key requires a JSON object payload to attach the key field"
    );
    assert!(non_object.source().is_none());

    let missing = PushPayloadAugmentError::MissingIdempotencyKey;
    assert_eq!(
        missing.to_string(),
        "exactly-once delivery semantics require an idempotency key but none was supplied"
    );
    assert!(missing.source().is_none());

    let json_err = serde_json::from_slice::<Value>(b"not json").unwrap_err();
    let json_expected = format!("push payload is not valid JSON: {json_err}");
    let wrapped = PushPayloadAugmentError::Json(json_err);
    assert_eq!(wrapped.to_string(), json_expected);
    assert!(wrapped.source().is_some());
}
