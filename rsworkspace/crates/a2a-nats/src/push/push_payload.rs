use std::borrow::Cow;

use serde_json::Value;

use crate::push::delivery_semantics::DeliverySemantics;
use crate::push::push_idempotency_key::PushIdempotencyKey;

pub const PUSH_IDEMPOTENCY_JSON_FIELD: &str = "_a2aPushIdempotencyKey";

pub fn augment_terminal_push_notification_bytes<'a>(
    base: &'a [u8],
    delivery_semantics: &DeliverySemantics,
    idempotency_key: Option<&PushIdempotencyKey>,
) -> Result<Cow<'a, [u8]>, serde_json::Error> {
    if delivery_semantics.idempotency_key_required() && idempotency_key.is_some() {
        let mut v: Value = serde_json::from_slice(base)?;
        if let Value::Object(ref mut map) = v
            && let Some(k) = idempotency_key
        {
            map.insert(
                PUSH_IDEMPOTENCY_JSON_FIELD.to_owned(),
                Value::String(k.as_str().to_owned()),
            );
        }
        Ok(Cow::Owned(serde_json::to_vec(&v)?))
    } else {
        Ok(Cow::Borrowed(base))
    }
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
    fn non_object_base_passes_through_unchanged_under_required_semantics() {
        let semantics = DeliverySemantics::ExactlyOnce {
            idempotency_key_header: None,
        };
        let base = b"[1,2,3]";
        let augmented = augment_terminal_push_notification_bytes(base, &semantics, Some(&key())).unwrap();
        let parsed: Value = serde_json::from_slice(&augmented).unwrap();
        assert!(parsed.is_array());
    }

    #[test]
    fn invalid_json_base_surfaces_serde_error() {
        let semantics = DeliverySemantics::ExactlyOnce {
            idempotency_key_header: None,
        };
        let base = b"not json";
        assert!(augment_terminal_push_notification_bytes(base, &semantics, Some(&key())).is_err());
    }
}
