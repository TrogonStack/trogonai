use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::constants::HTTP_PUSH_WEBHOOK_MAX_ATTEMPTS;
use crate::push::idempotency_key_header::{IdempotencyKeyHeader, IdempotencyKeyHeaderError};

/// Default [`Idempotency-Key`] carrier for webhook targets (see sketch).
pub static DEFAULT_WEBHOOK_IDEMPOTENCY_HEADER_NAME: http::HeaderName = http::HeaderName::from_static("idempotency-key");

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum DeliverySemantics {
    #[default]
    AtLeastOnce,
    ExactlyOnce {
        idempotency_key_header: Option<IdempotencyKeyHeader>,
    },
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum DeliverySemanticsParseError {
    UnknownShape,
    /// Both `atLeastOnce` and `exactlyOnce` keys present in the same object —
    /// caller intent is ambiguous, refuse to silently pick one.
    ConflictingShape,
    InvalidIdempotencyHeaderName(IdempotencyKeyHeaderError),
}

impl std::fmt::Display for DeliverySemanticsParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownShape => write!(
                f,
                r#"expected "atLeastOnce", "exactlyOnce", {{ "atLeastOnce": ... }}, or {{ "exactlyOnce": ... }}"#
            ),
            Self::ConflictingShape => write!(
                f,
                r#"deliverySemantics object carries both "atLeastOnce" and "exactlyOnce" keys"#
            ),
            Self::InvalidIdempotencyHeaderName(inner) => std::fmt::Display::fmt(inner, f),
        }
    }
}

impl std::error::Error for DeliverySemanticsParseError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidIdempotencyHeaderName(inner) => Some(inner),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct ExactlyOnceWire {
    // Accept both `idempotencyKeyHeader` and `idempotency_key_header` so callers
    // that picked snake-case on the outer object (we accept `exactly_once` as
    // well as `exactlyOnce`) can't lose the header through inner-shape drift.
    #[serde(default, alias = "idempotency_key_header")]
    idempotency_key_header: Option<String>,
}

pub fn merged_request_delivery_semantics(
    delivery_semantics_json: Option<&Value>,
    exactly_once_delivery_json: Option<bool>,
) -> Result<DeliverySemantics, DeliverySemanticsParseError> {
    if let Some(raw) = delivery_semantics_json {
        return parse_delivery_semantics_value(raw);
    }

    Ok(match exactly_once_delivery_json {
        Some(true) => DeliverySemantics::ExactlyOnce {
            idempotency_key_header: None,
        },
        Some(false) | None => DeliverySemantics::AtLeastOnce,
    })
}

pub fn parse_delivery_semantics_value(value: &Value) -> Result<DeliverySemantics, DeliverySemanticsParseError> {
    match value {
        Value::String(s) if s.eq_ignore_ascii_case("atleastonce") => Ok(DeliverySemantics::AtLeastOnce),
        Value::String(s) if s.eq_ignore_ascii_case("exactlyonce") => Ok(DeliverySemantics::ExactlyOnce {
            idempotency_key_header: None,
        }),
        Value::Object(map) => parse_delivery_semantics_object(map),
        _ => Err(DeliverySemanticsParseError::UnknownShape),
    }
}

fn parse_delivery_semantics_object(
    map: &serde_json::Map<String, Value>,
) -> Result<DeliverySemantics, DeliverySemanticsParseError> {
    let has_at_least_once = map.contains_key("atLeastOnce") || map.contains_key("at_least_once");
    let exactly = map.get("exactlyOnce").or_else(|| map.get("exactly_once"));

    match (has_at_least_once, exactly) {
        (true, Some(_)) => return Err(DeliverySemanticsParseError::ConflictingShape),
        (true, None) => return Ok(DeliverySemantics::AtLeastOnce),
        (false, None) => return Err(DeliverySemanticsParseError::UnknownShape),
        (false, Some(_)) => {}
    }

    let exactly = exactly.ok_or(DeliverySemanticsParseError::UnknownShape)?;

    match exactly {
        // `exactlyOnce: false` matches the legacy `exactlyOnceDelivery: false`
        // wire path and degrades to at-least-once; `null` / `true` opt into
        // exactly-once with the default header carrier.
        Value::Bool(false) => Ok(DeliverySemantics::AtLeastOnce),
        Value::Null | Value::Bool(true) => Ok(DeliverySemantics::ExactlyOnce {
            idempotency_key_header: None,
        }),
        inner @ Value::Object(_) => {
            let inner: ExactlyOnceWire =
                serde_json::from_value(inner.clone()).map_err(|_| DeliverySemanticsParseError::UnknownShape)?;
            let hdr = inner
                .idempotency_key_header
                .map(|raw| IdempotencyKeyHeader::try_from(raw.as_str()))
                .transpose()
                .map_err(DeliverySemanticsParseError::InvalidIdempotencyHeaderName)?;
            Ok(DeliverySemantics::ExactlyOnce {
                idempotency_key_header: hdr,
            })
        }
        _ => Err(DeliverySemanticsParseError::UnknownShape),
    }
}

pub fn upsert_delivery_semantics_on_push_config_json_object(
    map: &mut serde_json::Map<String, Value>,
    semantics: &DeliverySemantics,
) {
    match semantics {
        DeliverySemantics::AtLeastOnce => {
            map.remove("deliverySemantics");
        }
        DeliverySemantics::ExactlyOnce {
            idempotency_key_header: header,
        } => {
            let inner = ExactlyOnceWire {
                idempotency_key_header: header.as_ref().map(ToString::to_string),
            };
            // ExactlyOnceWire only holds an Option<String>; serde_json::to_value
            // can't fail at runtime, so fall back to the empty body if the
            // (impossible) error fires — never panic on push-config writes.
            let body = serde_json::to_value(inner).unwrap_or(Value::Object(Default::default()));
            map.insert(
                "deliverySemantics".to_owned(),
                serde_json::json!({ "exactlyOnce": body }),
            );
        }
    }
}

impl DeliverySemantics {
    pub fn webhook_max_publish_attempts(&self) -> u32 {
        HTTP_PUSH_WEBHOOK_MAX_ATTEMPTS
    }

    pub fn jetstream_max_publish_attempts(&self) -> u32 {
        HTTP_PUSH_WEBHOOK_MAX_ATTEMPTS
    }

    pub fn nats_core_publish_max_attempts(&self) -> u32 {
        match self {
            DeliverySemantics::AtLeastOnce => HTTP_PUSH_WEBHOOK_MAX_ATTEMPTS,
            DeliverySemantics::ExactlyOnce { .. } => 1,
        }
    }

    pub fn webhook_idempotency_carrier(&self) -> Option<&http::HeaderName> {
        match self {
            DeliverySemantics::AtLeastOnce => None,
            DeliverySemantics::ExactlyOnce { idempotency_key_header } => Some(
                idempotency_key_header
                    .as_ref()
                    .map(IdempotencyKeyHeader::as_http)
                    .unwrap_or(&DEFAULT_WEBHOOK_IDEMPOTENCY_HEADER_NAME),
            ),
        }
    }

    pub fn idempotency_key_required(&self) -> bool {
        matches!(self, Self::ExactlyOnce { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_at_least_once() {
        assert_eq!(DeliverySemantics::default(), DeliverySemantics::AtLeastOnce);
    }

    #[test]
    fn idempotency_key_required_only_for_exactly_once() {
        assert!(!DeliverySemantics::AtLeastOnce.idempotency_key_required());
        assert!(
            DeliverySemantics::ExactlyOnce {
                idempotency_key_header: None
            }
            .idempotency_key_required()
        );
    }

    #[test]
    fn webhook_max_publish_attempts_uses_constant() {
        let semantics = DeliverySemantics::AtLeastOnce;
        assert_eq!(semantics.webhook_max_publish_attempts(), HTTP_PUSH_WEBHOOK_MAX_ATTEMPTS);
        assert_eq!(
            semantics.jetstream_max_publish_attempts(),
            HTTP_PUSH_WEBHOOK_MAX_ATTEMPTS
        );
    }

    #[test]
    fn nats_core_publish_max_attempts_collapses_to_one_under_exactly_once() {
        assert_eq!(
            DeliverySemantics::AtLeastOnce.nats_core_publish_max_attempts(),
            HTTP_PUSH_WEBHOOK_MAX_ATTEMPTS
        );
        assert_eq!(
            DeliverySemantics::ExactlyOnce {
                idempotency_key_header: None
            }
            .nats_core_publish_max_attempts(),
            1
        );
    }

    #[test]
    fn webhook_idempotency_carrier_returns_default_when_unset_under_exactly_once() {
        let semantics = DeliverySemantics::ExactlyOnce {
            idempotency_key_header: None,
        };
        let carrier = semantics.webhook_idempotency_carrier().unwrap();
        assert_eq!(carrier.as_str(), "idempotency-key");
    }

    #[test]
    fn webhook_idempotency_carrier_uses_custom_header_when_set() {
        let header = IdempotencyKeyHeader::try_from("X-Push-Key").unwrap();
        let semantics = DeliverySemantics::ExactlyOnce {
            idempotency_key_header: Some(header),
        };
        let carrier = semantics.webhook_idempotency_carrier().unwrap();
        assert_eq!(carrier.as_str(), "x-push-key");
    }

    #[test]
    fn webhook_idempotency_carrier_is_none_for_at_least_once() {
        assert!(DeliverySemantics::AtLeastOnce.webhook_idempotency_carrier().is_none());
    }

    #[test]
    fn merged_uses_delivery_semantics_when_supplied() {
        let merged = merged_request_delivery_semantics(Some(&serde_json::json!("exactlyOnce")), Some(false)).unwrap();
        assert!(merged.idempotency_key_required());
    }

    #[test]
    fn merged_falls_back_to_exactly_once_delivery_bool() {
        let merged = merged_request_delivery_semantics(None, Some(true)).unwrap();
        assert!(merged.idempotency_key_required());
    }

    #[test]
    fn merged_defaults_to_at_least_once_when_neither_set() {
        assert_eq!(
            merged_request_delivery_semantics(None, None).unwrap(),
            DeliverySemantics::AtLeastOnce
        );
        assert_eq!(
            merged_request_delivery_semantics(None, Some(false)).unwrap(),
            DeliverySemantics::AtLeastOnce
        );
    }

    #[test]
    fn parse_value_string_atleastonce_is_case_insensitive() {
        for raw in ["atLeastOnce", "ATLEASTONCE", "atleastonce"] {
            let parsed = parse_delivery_semantics_value(&serde_json::json!(raw)).unwrap();
            assert_eq!(parsed, DeliverySemantics::AtLeastOnce);
        }
    }

    #[test]
    fn parse_value_string_exactlyonce_is_case_insensitive() {
        for raw in ["exactlyOnce", "EXACTLYONCE", "exactlyonce"] {
            let parsed = parse_delivery_semantics_value(&serde_json::json!(raw)).unwrap();
            assert!(parsed.idempotency_key_required());
        }
    }

    #[test]
    fn parse_value_rejects_unknown_string_or_primitive() {
        assert!(matches!(
            parse_delivery_semantics_value(&serde_json::json!("unknown")),
            Err(DeliverySemanticsParseError::UnknownShape)
        ));
        assert!(matches!(
            parse_delivery_semantics_value(&serde_json::json!(42)),
            Err(DeliverySemanticsParseError::UnknownShape)
        ));
    }

    #[test]
    fn parse_object_atleastonce_in_camel_and_snake() {
        let parsed = parse_delivery_semantics_value(&serde_json::json!({"atLeastOnce": {}})).unwrap();
        assert_eq!(parsed, DeliverySemantics::AtLeastOnce);
        let parsed = parse_delivery_semantics_value(&serde_json::json!({"at_least_once": null})).unwrap();
        assert_eq!(parsed, DeliverySemantics::AtLeastOnce);
    }

    #[test]
    fn parse_object_exactlyonce_with_null_or_bool_yields_default_header() {
        let parsed = parse_delivery_semantics_value(&serde_json::json!({"exactlyOnce": null})).unwrap();
        assert_eq!(
            parsed,
            DeliverySemantics::ExactlyOnce {
                idempotency_key_header: None
            }
        );
        let parsed = parse_delivery_semantics_value(&serde_json::json!({"exactlyOnce": true})).unwrap();
        assert_eq!(
            parsed,
            DeliverySemantics::ExactlyOnce {
                idempotency_key_header: None
            }
        );
    }

    #[test]
    fn parse_object_exactlyonce_inner_object_parses_header() {
        let parsed =
            parse_delivery_semantics_value(&serde_json::json!({"exactlyOnce": {"idempotencyKeyHeader": "X-Push-Key"}}))
                .unwrap();
        let expected = DeliverySemantics::ExactlyOnce {
            idempotency_key_header: Some(IdempotencyKeyHeader::try_from("X-Push-Key").unwrap()),
        };
        assert_eq!(parsed, expected);
    }

    #[test]
    fn parse_object_exactlyonce_rejects_bad_header_name() {
        let err = parse_delivery_semantics_value(
            &serde_json::json!({"exactlyOnce": {"idempotencyKeyHeader": "not a valid header"}}),
        )
        .unwrap_err();
        assert!(matches!(
            err,
            DeliverySemanticsParseError::InvalidIdempotencyHeaderName(_)
        ));
    }

    #[test]
    fn parse_object_exactlyonce_rejects_non_object_non_null_non_bool() {
        let err = parse_delivery_semantics_value(&serde_json::json!({"exactlyOnce": 42})).unwrap_err();
        assert!(matches!(err, DeliverySemanticsParseError::UnknownShape));
    }

    #[test]
    fn parse_object_without_known_keys_is_unknown_shape() {
        let err = parse_delivery_semantics_value(&serde_json::json!({"foo": "bar"})).unwrap_err();
        assert!(matches!(err, DeliverySemanticsParseError::UnknownShape));
    }

    #[test]
    fn parse_object_with_both_at_least_once_and_exactly_once_is_conflicting() {
        let err =
            parse_delivery_semantics_value(&serde_json::json!({"atLeastOnce": {}, "exactlyOnce": null})).unwrap_err();
        assert!(matches!(err, DeliverySemanticsParseError::ConflictingShape));
    }

    #[test]
    fn parse_object_exactlyonce_false_degrades_to_at_least_once() {
        let parsed = parse_delivery_semantics_value(&serde_json::json!({"exactlyOnce": false})).unwrap();
        assert_eq!(parsed, DeliverySemantics::AtLeastOnce);
        let parsed = parse_delivery_semantics_value(&serde_json::json!({"exactly_once": false})).unwrap();
        assert_eq!(parsed, DeliverySemantics::AtLeastOnce);
    }

    #[test]
    fn parse_object_exactlyonce_inner_object_parses_snake_case_header() {
        let parsed = parse_delivery_semantics_value(
            &serde_json::json!({"exactlyOnce": {"idempotency_key_header": "X-Push-Key"}}),
        )
        .unwrap();
        let expected = DeliverySemantics::ExactlyOnce {
            idempotency_key_header: Some(IdempotencyKeyHeader::try_from("X-Push-Key").unwrap()),
        };
        assert_eq!(parsed, expected);
    }

    #[test]
    fn parse_object_with_snake_case_both_keys_is_conflicting() {
        let err = parse_delivery_semantics_value(&serde_json::json!({"at_least_once": null, "exactly_once": true}))
            .unwrap_err();
        assert!(matches!(err, DeliverySemanticsParseError::ConflictingShape));
    }

    #[test]
    fn upsert_at_least_once_removes_field() {
        let mut map = serde_json::Map::new();
        map.insert("deliverySemantics".into(), serde_json::json!({"existing": "x"}));
        upsert_delivery_semantics_on_push_config_json_object(&mut map, &DeliverySemantics::AtLeastOnce);
        assert!(!map.contains_key("deliverySemantics"));
    }

    #[test]
    fn upsert_exactly_once_writes_field() {
        let mut map = serde_json::Map::new();
        upsert_delivery_semantics_on_push_config_json_object(
            &mut map,
            &DeliverySemantics::ExactlyOnce {
                idempotency_key_header: None,
            },
        );
        assert!(map["deliverySemantics"]["exactlyOnce"].is_object());
    }

    #[test]
    fn upsert_exactly_once_writes_custom_header() {
        let mut map = serde_json::Map::new();
        let header = IdempotencyKeyHeader::try_from("X-Push-Key").unwrap();
        upsert_delivery_semantics_on_push_config_json_object(
            &mut map,
            &DeliverySemantics::ExactlyOnce {
                idempotency_key_header: Some(header),
            },
        );
        assert_eq!(
            map["deliverySemantics"]["exactlyOnce"]["idempotencyKeyHeader"]
                .as_str()
                .unwrap(),
            "x-push-key"
        );
    }

    #[test]
    fn parse_error_display_covers_every_variant() {
        use std::error::Error as _;
        let unknown = DeliverySemanticsParseError::UnknownShape;
        assert!(unknown.to_string().contains("atLeastOnce"));
        assert!(unknown.source().is_none());

        let conflicting = DeliverySemanticsParseError::ConflictingShape;
        assert!(conflicting.to_string().contains("both"));
        assert!(conflicting.source().is_none());

        let invalid = DeliverySemanticsParseError::InvalidIdempotencyHeaderName(
            IdempotencyKeyHeader::try_from("not a valid header").unwrap_err(),
        );
        assert!(invalid.source().is_some());
        assert!(!invalid.to_string().is_empty());
    }
}
