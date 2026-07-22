use std::borrow::Cow;

use serde_json::Value;

use crate::push::delivery_semantics::DeliverySemantics;
use crate::push::push_idempotency_key::PushIdempotencyKey;

pub const PUSH_IDEMPOTENCY_JSON_FIELD: &str = "_a2aPushIdempotencyKey";

#[derive(Debug, thiserror::Error)]
pub enum PushPayloadAugmentError {
    /// Underlying payload bytes are not valid JSON.
    #[error("push payload is not valid JSON: {0}")]
    Json(#[source] serde_json::Error),
    /// Caller asked for an exactly-once idempotency key on a payload whose
    /// top-level JSON value isn't an object — there's nowhere to attach the
    /// key without rewriting the payload shape, so the dedup contract would
    /// be silently lost on the wire.
    #[error("exactly-once idempotency key requires a JSON object payload to attach the key field")]
    NonObjectPayloadForIdempotencyKey,
    /// Delivery semantics require an idempotency key (exactly-once) but the
    /// caller didn't supply one — refusing to augment would silently downgrade
    /// to at-least-once on the wire.
    #[error("exactly-once delivery semantics require an idempotency key but none was supplied")]
    MissingIdempotencyKey,
}

pub fn augment_terminal_push_notification_bytes<'a>(
    base: &'a [u8],
    delivery_semantics: &DeliverySemantics,
    idempotency_key: Option<&PushIdempotencyKey>,
) -> Result<Cow<'a, [u8]>, PushPayloadAugmentError> {
    if !delivery_semantics.idempotency_key_required() {
        // At-least-once: idempotency key is optional and the payload is shipped
        // verbatim regardless of whether the caller passed one.
        return Ok(Cow::Borrowed(base));
    }

    let key = idempotency_key.ok_or(PushPayloadAugmentError::MissingIdempotencyKey)?;

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
mod tests;
