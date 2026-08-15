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
/// Deliberately more permissive than [`Message`]: a public HTTP edge has to
/// answer a malformed request with a JSON-RPC error that echoes what it can,
/// where the canonical codec can only reject the whole envelope.
#[derive(Debug, Deserialize)]
pub struct InboundRequest {
    jsonrpc: Option<String>,
    id: Option<Value>,
    pub method: String,
    params: Option<Value>,
}

impl InboundRequest {
    /// Whether the caller declared the one JSON-RPC version this edge speaks.
    pub fn has_supported_version(&self) -> bool {
        self.jsonrpc.as_deref() == Some(JSONRPC_VERSION)
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
