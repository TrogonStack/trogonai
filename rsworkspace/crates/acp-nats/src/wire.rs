//! JSON-RPC content-mode wire helpers for ACP over NATS.

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

/// Encode a JSON-RPC notification (params in body, no `Jsonrpc-Id`).
pub fn encode_notification<Req: Serialize>(method: &str, params: &Req) -> Result<Encoded, WireError> {
    let params = serde_json::to_value(params).map_err(WireError::Serialize)?;
    encode(&Message::Notification {
        method: method.to_string(),
        params,
    })
    .map_err(WireError::from)
}

/// Decode notification params from a content-mode NATS message.
pub fn decode_notification_params<Req: DeserializeOwned>(
    method: &str,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Req, WireError> {
    match decode(Direction::Request, Some(method), headers, body)? {
        Message::Notification { params, .. } => serde_json::from_value(params).map_err(WireError::Deserialize),
        _ => Err(WireError::UnexpectedMessage),
    }
}

pub fn merge_jsonrpc_headers(base: HeaderMap, overlay: HeaderMap) -> HeaderMap {
    jsonrpc_nats::merge_jsonrpc_headers(base, overlay)
}

pub fn acp_request_id(id: &ResponseId) -> agent_client_protocol::RequestId {
    match id {
        ResponseId::Number(n) => agent_client_protocol::RequestId::Number(*n),
        ResponseId::String(s) => agent_client_protocol::RequestId::Str(s.clone()),
        ResponseId::Null => agent_client_protocol::RequestId::Null,
    }
}

pub fn response_id_from_acp(id: &agent_client_protocol::RequestId) -> ResponseId {
    match id {
        agent_client_protocol::RequestId::Number(n) => ResponseId::Number(*n),
        agent_client_protocol::RequestId::Str(s) => ResponseId::String(s.clone()),
        agent_client_protocol::RequestId::Null => ResponseId::Null,
    }
}

/// Encode a JSON-RPC request call (params in body, id in `Jsonrpc-Id`).
pub fn encode_request<Req: Serialize>(method: &str, id: RequestId, params: &Req) -> Result<Encoded, WireError> {
    let params = serde_json::to_value(params).map_err(WireError::Serialize)?;
    encode(&Message::Request {
        id,
        method: method.to_string(),
        params,
    })
    .map_err(WireError::from)
}

/// Encode a success response (result in body).
pub fn encode_success<Res: Serialize>(id: ResponseId, result: &Res) -> Result<Encoded, WireError> {
    let result = serde_json::to_value(result).map_err(WireError::Serialize)?;
    encode(&Message::Success { id, result }).map_err(WireError::from)
}

/// Encode an agent error response (`Jsonrpc-Error-Code` + message body).
pub fn encode_agent_error(id: ResponseId, error: &agent_client_protocol::Error) -> Result<Encoded, WireError> {
    encode(&Message::Error {
        id,
        code: i32::from(error.code),
        message: error.message.clone(),
        data: error.data.clone(),
    })
    .map_err(WireError::from)
}

/// Decode a JSON-RPC response into a typed result or structured agent error.
pub fn decode_response<Res: DeserializeOwned>(
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Result<Res, agent_client_protocol::Error>, WireError> {
    match decode(Direction::Response, None, headers, body)? {
        Message::Success { result, .. } => {
            let value = serde_json::from_value(result).map_err(WireError::Deserialize)?;
            Ok(Ok(value))
        }
        Message::Error {
            code, message, data, ..
        } => Ok(Err(agent_client_protocol::Error::new(code, message).data(data))),
        _ => Err(WireError::UnexpectedMessage),
    }
}

/// Decode a JSON-RPC request params body.
pub fn decode_request_params<Req: DeserializeOwned>(
    method: &str,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Req, WireError> {
    match decode(Direction::Request, Some(method), headers, body)? {
        Message::Request { params, .. } => serde_json::from_value(params).map_err(WireError::Deserialize),
        _ => Err(WireError::UnexpectedMessage),
    }
}

/// Extract the response id from request headers for reply correlation.
pub fn response_id_from_request_headers(headers: &HeaderMap) -> ResponseId {
    match headers.get(jsonrpc_nats::HEADER_ID) {
        Some(value) => jsonrpc_nats::decode_response_id_literal(value.as_str()).unwrap_or(ResponseId::Null),
        None => ResponseId::Null,
    }
}

#[cfg(test)]
mod tests;
