use serde::Serialize;
use serde_json::Value;

use crate::constants::JSONRPC_VERSION;

/// The `error` member of a JSON-RPC response.
#[derive(Debug, Clone, Serialize)]
pub struct RpcError {
    pub code: i32,
    pub message: String,
}

impl RpcError {
    pub fn new(code: i32, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

/// JSON-RPC error response.
///
/// The HTTP edge echoes the caller's `id` exactly as it arrived, so it stays a
/// [`Value`] rather than a narrowed id type: a caller that sent a malformed id
/// still has to be able to correlate the rejection.
#[derive(Debug, Clone, Serialize)]
pub struct OutboundError {
    jsonrpc: &'static str,
    id: Value,
    error: RpcError,
}

impl OutboundError {
    pub fn new(id: Value, code: i32, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION,
            id,
            error: RpcError::new(code, message),
        }
    }
}

/// JSON-RPC success response wrapping a typed result.
#[derive(Debug, Clone, Serialize)]
pub struct OutboundResult<T> {
    jsonrpc: &'static str,
    id: Value,
    result: T,
}

impl<T> OutboundResult<T> {
    pub fn new(id: Value, result: T) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION,
            id,
            result,
        }
    }
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
            error: RpcError::new(code, message),
        }
    }
}
