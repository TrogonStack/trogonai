//! JSON-RPC envelope helpers for the A2A binding.
//!
//! A2A method dispatch lives at the NATS subject level — one subject per method, so the
//! JSON-RPC `method` field is redundant on the wire and we don't require it. What we do
//! need: extract the request `id` from NATS headers without paying full deserialization
//! cost, so we can route responses back to the right caller inbox.

use async_nats::header::HeaderMap;
use jsonrpc_nats::ResponseId;
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

impl From<ResponseId> for JsonRpcId {
    fn from(id: ResponseId) -> Self {
        match id {
            ResponseId::Number(n) => Self::Number(n),
            ResponseId::String(s) => Self::String(s),
            ResponseId::Null => Self::Null,
        }
    }
}

/// Extracts the JSON-RPC id from `Jsonrpc-Id` request headers.
///
/// Returns `None` when the header is absent (notification). Returns `Some(JsonRpcId::Null)`
/// when the header carries the JSON literal `null`.
pub fn extract_request_id(headers: &HeaderMap) -> Option<JsonRpcId> {
    let value = headers.get(jsonrpc_nats::HEADER_ID)?.as_str();
    jsonrpc_nats::decode_response_id_literal(value)
        .ok()
        .map(JsonRpcId::from)
}

/// Legacy body-based id extraction retained for transitional call sites that only
/// have a payload hint (e.g. gateway ingress error helpers before headers arrive).
pub fn extract_request_id_from_body(raw: &[u8]) -> Option<JsonRpcId> {
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
