use std::borrow::Cow;

use serde_json::Value;

use crate::push::delivery_semantics::DeliverySemantics;
use crate::push::push_idempotency_key::PushIdempotencyKey;

const PUSH_IDEMPOTENCY_JSON_FIELD: &str = "_a2aPushIdempotencyKey";

pub(crate) fn augment_terminal_push_notification_bytes<'a>(
    base: &'a [u8],
    delivery_semantics: &DeliverySemantics,
    idempotency_key: Option<&PushIdempotencyKey>,
) -> Result<Cow<'a, [u8]>, serde_json::Error> {
    if delivery_semantics.idempotency_key_required() && idempotency_key.is_some() {
        let mut v: Value = serde_json::from_slice(base)?;
        if let Value::Object(ref mut map) = v && let Some(k) = idempotency_key {
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
