//! JSON-RPC envelope helpers for the A2A binding.
//!
//! A2A method dispatch lives at the NATS subject level — one subject per method, so the
//! JSON-RPC `method` field is redundant on the wire and we don't require it. What we do
//! need: extract the request `id` from NATS headers without paying full deserialization
//! cost, so we can route responses back to the right caller inbox.

use async_nats::header::HeaderMap;
use jsonrpc_nats::ResponseId;
use serde_json::Value;

/// Extracts the JSON-RPC id from `Jsonrpc-Id` request headers.
///
/// Returns `None` when the header is absent (notification). Returns `Some(ResponseId::Null)`
/// when the header carries the JSON literal `null`.
pub fn extract_request_id(headers: &HeaderMap) -> Option<ResponseId> {
    let value = headers.get(jsonrpc_nats::HEADER_ID)?.as_str();
    jsonrpc_nats::decode_response_id_literal(value).ok()
}

/// Legacy body-based id extraction retained for transitional call sites that only
/// have a payload hint (e.g. gateway ingress error helpers before headers arrive).
pub fn extract_request_id_from_body(raw: &[u8]) -> Option<ResponseId> {
    let value: Value = serde_json::from_slice(raw).ok()?;
    let id = value.as_object()?.get("id")?;
    match id {
        Value::Number(n) => n.as_i64().map(ResponseId::Number),
        Value::String(s) => Some(ResponseId::String(s.clone())),
        Value::Null => Some(ResponseId::Null),
        _ => None,
    }
}

/// Canonical correlation key for the id in a request body: the id's JSON
/// literal, the same form the `Jsonrpc-Id` header carries.
///
/// The literal keeps the id's type in the key, so a numeric `7` and a string
/// `"7"` cannot collapse onto one stream pump or one audit row. A missing id
/// and a `null` id both yield `None`: neither can correlate anything, and a
/// synthesized token would alias unrelated envelopes together.
pub fn correlation_key_from_body(raw: &[u8]) -> Option<String> {
    jsonrpc_nats::encode_response_id_literal(&extract_request_id_from_body(raw)?)
}

#[cfg(test)]
mod tests;
