//! JSON-RPC framing for the HTTP edge.
//!
//! Outbound envelopes come from [`jsonrpc_nats`], the same codec the NATS side
//! encodes with (ADR#0056), so the two ends of a bridged call cannot drift into
//! different renderings of `{jsonrpc, id, result|error}`.

use jsonrpc_nats::{JSONRPC_VERSION, Message, ResponseId, to_json_value};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Inbound JSON-RPC request exactly as it arrived, before validation.
///
/// Every member stays a [`Value`] so that deserialization of a syntactically
/// valid JSON body always succeeds: shape is a JSON-RPC concern answered with
/// `-32600` and the caller's id, not something to hand off to the HTTP layer's
/// own rejection. That makes this deliberately more permissive than [`Message`],
/// which can only accept or reject the envelope whole.
#[derive(Debug, Deserialize)]
pub struct InboundRequest {
    jsonrpc: Option<Value>,
    id: Option<Value>,
    method: Option<Value>,
    params: Option<Value>,
}

impl InboundRequest {
    /// Whether the caller declared the one JSON-RPC version this edge speaks.
    pub fn has_supported_version(&self) -> bool {
        matches!(self.jsonrpc.as_ref(), Some(Value::String(version)) if version == JSONRPC_VERSION)
    }

    /// Method to dispatch on, or [`None`] when the member is absent or not a string.
    pub fn method(&self) -> Option<&str> {
        self.method.as_ref()?.as_str()
    }

    /// Id to correlate responses against, coerced to a canonical response id.
    pub fn response_id(&self) -> ResponseId {
        self.id
            .as_ref()
            .map_or(ResponseId::Null, ResponseId::from_request_value)
    }

    /// Params to dispatch on; an omitted `params` member reads as `null`.
    pub fn params(&mut self) -> Value {
        self.params.take().unwrap_or(Value::Null)
    }
}

/// JSON-RPC success response wrapping a typed result.
///
/// Fails only if `result` is not representable as JSON; callers answer that with
/// [`error`] carrying [`jsonrpc_nats::INTERNAL_ERROR`].
pub fn success<T: Serialize>(id: &ResponseId, result: &T) -> Result<Value, serde_json::Error> {
    Ok(to_json_value(&Message::Success {
        id: id.clone(),
        result: serde_json::to_value(result)?,
    }))
}

/// JSON-RPC error response.
pub fn error(id: &ResponseId, code: i32, message: impl Into<String>) -> Value {
    to_json_value(&Message::Error {
        id: id.clone(),
        code,
        message: message.into(),
        data: None,
    })
}

/// The `error` member of a JSON-RPC response, on its own.
#[derive(Debug, Clone, Serialize)]
pub struct RpcError {
    pub code: i32,
    pub message: String,
}

/// REST failure body: the `error` member without a JSON-RPC envelope, since
/// REST routes carry no request id to correlate against.
#[derive(Debug, Clone, Serialize)]
pub struct RestError {
    error: RpcError,
}

impl RestError {
    pub fn new(code: i32, message: impl Into<String>) -> Self {
        Self {
            error: RpcError {
                code,
                message: message.into(),
            },
        }
    }
}

#[cfg(test)]
mod tests;
