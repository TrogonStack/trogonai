use serde::Serialize;
use serde_json::Value;

use crate::DispatchError;

/// Serializes a tool result into the `result` of a response frame.
pub(crate) fn encode<T: Serialize>(value: T) -> Result<Value, DispatchError> {
    Ok(serde_json::to_value(value)?)
}
