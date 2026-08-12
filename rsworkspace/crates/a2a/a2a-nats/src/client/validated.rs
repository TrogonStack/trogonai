//! Typed decode plus retained canonical body bytes (ADR#0021 + ADR#0056).
//!
//! Bridges validate at the NATS boundary, then forward the response envelope
//! without hand-assembling `{jsonrpc,id,result|error}`.

use bytes::Bytes;
use serde_json::Value;

/// Domain value after typed decode, with the validated NATS body retained.
#[derive(Debug, Clone)]
pub struct ValidatedRpc<T> {
    pub value: T,
    /// Canonical JSON-RPC response body that produced `value`.
    pub body: Bytes,
}

impl<T> ValidatedRpc<T> {
    pub fn new(value: T, body: Bytes) -> Self {
        Self { value, body }
    }

    /// Rewrite the JSON-RPC `id` in a validated response body to the edge client's id.
    ///
    /// Transport request ids often differ from the stdio/HTTP client's id; the
    /// envelope otherwise stays unmodified.
    pub fn body_with_client_id(&self, client_id: &Value) -> Result<Bytes, serde_json::Error> {
        rewrite_response_id(&self.body, client_id)
    }
}

/// Replace top-level `"id"` in a canonical JSON-RPC response body.
pub fn rewrite_response_id(body: &[u8], client_id: &Value) -> Result<Bytes, serde_json::Error> {
    let mut value: Value = serde_json::from_slice(body)?;
    if let Some(object) = value.as_object_mut() {
        object.insert("id".to_string(), client_id.clone());
    }
    Ok(Bytes::from(serde_json::to_vec(&value)?))
}
