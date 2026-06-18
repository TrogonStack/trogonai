use std::borrow::Cow;
use std::fmt;

use serde_json::Value;

use crate::push::delivery_semantics::DeliverySemantics;
use crate::push::push_idempotency_key::PushIdempotencyKey;

pub const PUSH_IDEMPOTENCY_JSON_FIELD: &str = "_a2aPushIdempotencyKey";

#[derive(Debug)]
pub enum PushPayloadAugmentError {
    /// Underlying payload bytes are not valid JSON.
    Json(serde_json::Error),
    /// Caller asked for an exactly-once idempotency key on a payload whose
    /// top-level JSON value isn't an object — there's nowhere to attach the
    /// key without rewriting the payload shape, so the dedup contract would
    /// be silently lost on the wire.
    NonObjectPayloadForIdempotencyKey,
}

impl fmt::Display for PushPayloadAugmentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Json(e) => write!(f, "push payload is not valid JSON: {e}"),
            Self::NonObjectPayloadForIdempotencyKey => write!(
                f,
                "exactly-once idempotency key requires a JSON object payload to attach the key field"
            ),
        }
    }
}

impl std::error::Error for PushPayloadAugmentError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Json(e) => Some(e),
            Self::NonObjectPayloadForIdempotencyKey => None,
        }
    }
}

pub fn augment_terminal_push_notification_bytes<'a>(
    base: &'a [u8],
    delivery_semantics: &DeliverySemantics,
    idempotency_key: Option<&PushIdempotencyKey>,
) -> Result<Cow<'a, [u8]>, PushPayloadAugmentError> {
    let (Some(key), true) = (idempotency_key, delivery_semantics.idempotency_key_required()) else {
        return Ok(Cow::Borrowed(base));
    };

    let mut value: Value = serde_json::from_slice(base).map_err(PushPayloadAugmentError::Json)?;
    let Value::Object(ref mut map) = value else {
        return Err(PushPayloadAugmentError::NonObjectPayloadForIdempotencyKey);
    };
    map.insert(
        PUSH_IDEMPOTENCY_JSON_FIELD.to_owned(),
        Value::String(key.as_str().to_owned()),
    );
    let bytes = serde_json::to_vec(&value).map_err(PushPayloadAugmentError::Json)?;
    Ok(Cow::Owned(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::push::push_notification_config_id::PushNotificationConfigId;
    use crate::push::terminal_push_task_state::TerminalPushTaskState;
    use crate::task_id::A2aTaskId;

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
    fn pass_through_when_no_idempotency_key_supplied() {
        let semantics = DeliverySemantics::default();
        let base = br#"{"hello":"world"}"#;
        let augmented = augment_terminal_push_notification_bytes(base, &semantics, None).unwrap();
        assert!(matches!(augmented, Cow::Borrowed(_)));
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
        use std::error::Error as _;
        let non_object = PushPayloadAugmentError::NonObjectPayloadForIdempotencyKey;
        assert!(non_object.to_string().contains("JSON object payload"));
        assert!(non_object.source().is_none());

        let json_err = serde_json::from_slice::<Value>(b"not json").unwrap_err();
        let wrapped = PushPayloadAugmentError::Json(json_err);
        assert!(wrapped.to_string().contains("not valid JSON"));
        assert!(wrapped.source().is_some());
    }
}
