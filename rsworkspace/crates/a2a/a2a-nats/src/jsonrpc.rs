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

/// Correlation key for the id in a request body, in the one form the rest of
/// the transport already agrees on: the id's text, unquoted.
///
/// This is deliberately *not* the `Jsonrpc-Id` literal. `Trogon-Req-Id` is what
/// a stream pump filters on and what an audit row joins against, and the two
/// places that mint it (the bridge's caller-id derivation and the agent's event
/// stamp) both write a string id as its bare text. A key that quoted the id
/// would match neither, so every event would look like another request's and
/// every audit row would join nothing.
///
/// A missing id and a `null` id both yield `None`: neither can correlate
/// anything, and a synthesized token would alias unrelated envelopes together.
pub fn correlation_key_from_body(raw: &[u8]) -> Option<String> {
    match extract_request_id_from_body(raw)? {
        ResponseId::Null => None,
        ResponseId::Number(n) => Some(n.to_string()),
        ResponseId::String(s) => Some(s),
    }
}

#[cfg(test)]
mod tests;
