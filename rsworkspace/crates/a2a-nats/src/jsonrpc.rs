//! JSON-RPC envelope helpers for the A2A binding.
//!
//! A2A method dispatch lives at the NATS subject level — one subject per method, so the
//! JSON-RPC `method` field is redundant on the wire and we don't require it. What we do
//! need: extract the request `id` from a raw payload without paying full deserialization
//! cost, so we can route responses back to the right caller inbox.

use serde_json::Value;

/// Minimal JSON-RPC id, mirroring the subset A2A uses.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(untagged)]
pub enum JsonRpcId {
    Number(i64),
    String(String),
    Null,
}

impl std::fmt::Display for JsonRpcId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Number(n) => write!(f, "{n}"),
            Self::String(s) => f.write_str(s),
            Self::Null => f.write_str("null"),
        }
    }
}

/// Extracts the JSON-RPC id from a raw payload, returning `None` if absent or malformed.
///
/// Distinguishes `id: null` (valid JSON-RPC, returns `Some(JsonRpcId::Null)`) from
/// `id` absent entirely (returns `None`).
pub fn extract_request_id(raw: &[u8]) -> Option<JsonRpcId> {
    let value: Value = serde_json::from_slice(raw).ok()?;
    let id = value.as_object()?.get("id")?;
    match id {
        Value::Number(n) => n.as_i64().map(JsonRpcId::Number),
        Value::String(s) => Some(JsonRpcId::String(s.clone())),
        Value::Null => Some(JsonRpcId::Null),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
