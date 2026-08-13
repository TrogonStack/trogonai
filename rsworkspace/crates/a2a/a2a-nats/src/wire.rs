//! Canonical JSON-RPC wire helpers for A2A over NATS (ADR#0056).
//!
//! The NATS body carries the complete JSON-RPC 2.0 object. `Jsonrpc-Id` and
//! `Jsonrpc-Error-Code` are non-authoritative projections of the body. Per-message
//! signing is not implemented in this crate yet; the canonical body is the payload
//! digest input required by ADR#0051 (subject and operation remain separate claims).

use async_nats::header::HeaderMap;
use jsonrpc_nats::{CodecError, Direction, Encoded, Message, RequestId, ResponseId, decode, encode};
use serde::{Serialize, de::DeserializeOwned};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum WireError {
    #[error(transparent)]
    Codec(#[from] CodecError),
    #[error("failed to serialize params")]
    Serialize(#[source] serde_json::Error),
    #[error("failed to deserialize result")]
    Deserialize(#[source] serde_json::Error),
    #[error("unexpected JSON-RPC message variant")]
    UnexpectedMessage,
}

pub fn merge_jsonrpc_headers(base: HeaderMap, overlay: HeaderMap) -> HeaderMap {
    jsonrpc_nats::merge_jsonrpc_headers(base, overlay)
}

pub fn response_id_from_request_headers(headers: &HeaderMap) -> ResponseId {
    match headers.get(jsonrpc_nats::HEADER_ID) {
        Some(value) => jsonrpc_nats::decode_response_id_literal(value.as_str()).unwrap_or(ResponseId::Null),
        None => ResponseId::Null,
    }
}

/// True when the inbound request omits `Jsonrpc-Id` (JSON-RPC notification).
pub fn is_notification(headers: &HeaderMap) -> bool {
    headers.get(jsonrpc_nats::HEADER_ID).is_none()
}

/// Encode a JSON-RPC request (complete body; id projected to `Jsonrpc-Id`).
pub fn encode_request<Req: Serialize>(method: &str, id: RequestId, params: &Req) -> Result<Encoded, WireError> {
    let params = serde_json::to_value(params).map_err(WireError::Serialize)?;
    encode(&Message::Request {
        id,
        method: method.to_string(),
        params,
    })
    .map_err(WireError::from)
}

/// Encode a success response (complete body).
pub fn encode_success<Res: Serialize>(id: ResponseId, result: &Res) -> Result<Encoded, WireError> {
    let result = serde_json::to_value(result).map_err(WireError::Serialize)?;
    encode(&Message::Success { id, result }).map_err(WireError::from)
}

/// Encode an error response (complete body; code projected to `Jsonrpc-Error-Code`).
pub fn encode_error(
    id: ResponseId,
    code: i32,
    message: impl Into<String>,
    data: Option<serde_json::Value>,
) -> Result<Encoded, WireError> {
    encode(&Message::Error {
        id,
        code,
        message: message.into(),
        data,
    })
    .map_err(WireError::from)
}

/// Decode a JSON-RPC response into a typed result or structured error `(code, message)`.
pub fn decode_response<Res: DeserializeOwned>(
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Result<Res, (i32, String)>, WireError> {
    match decode(Direction::Response, None, headers, body)? {
        Message::Success { result, .. } => {
            let value = serde_json::from_value(result).map_err(WireError::Deserialize)?;
            Ok(Ok(value))
        }
        Message::Error { code, message, .. } => Ok(Err((code, message))),
        _ => Err(WireError::UnexpectedMessage),
    }
}

/// Decode a JSON-RPC request params body.
///
/// `method` is the A2A protocol method expected in the body (slash form).
pub fn decode_request_params<Req: DeserializeOwned>(
    method: &str,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Req, WireError> {
    match decode(Direction::Request, Some(method), headers, body)? {
        Message::Request { params, .. } => serde_json::from_value(params).map_err(WireError::Deserialize),
        Message::Notification { params, .. } => serde_json::from_value(params).map_err(WireError::Deserialize),
        _ => Err(WireError::UnexpectedMessage),
    }
}

#[cfg(test)]
mod tests;
