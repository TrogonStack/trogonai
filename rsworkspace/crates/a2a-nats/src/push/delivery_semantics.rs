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

#[derive(Debug, Clone, Eq, PartialEq, thiserror::Error)]
pub enum DeliverySemanticsParseError {
    #[error(r#"expected "atLeastOnce", "exactlyOnce", {{ "atLeastOnce": ... }}, or {{ "exactlyOnce": ... }}"#)]
    UnknownShape,
    /// Both `atLeastOnce` and `exactlyOnce` keys present in the same object —
    /// caller intent is ambiguous, refuse to silently pick one.
    #[error(r#"deliverySemantics object carries both "atLeastOnce" and "exactlyOnce" keys"#)]
    ConflictingShape,
    #[error("{0}")]
    InvalidIdempotencyHeaderName(#[source] IdempotencyKeyHeaderError),
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
mod tests;
