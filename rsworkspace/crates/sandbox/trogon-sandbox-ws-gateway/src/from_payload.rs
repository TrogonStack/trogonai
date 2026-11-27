use std::time::Duration;

use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::PayloadError;

/// Parses `payload` into the typed request `T`.
pub(crate) fn from_payload<T: DeserializeOwned>(payload: &Value) -> Result<T, PayloadError> {
    Ok(T::deserialize(payload)?)
}

/// The `timeout_seconds` field as a duration, or `default` when absent or not an unsigned integer.
pub(crate) fn timeout_or(payload: &Value, default: Duration) -> Duration {
    payload
        .get("timeout_seconds")
        .and_then(Value::as_u64)
        .map_or(default, Duration::from_secs)
}
