use async_nats::header::HeaderMap;
use bytes::Bytes;

use crate::constants::{HEADER_ERROR_CODE, HEADER_ID};
use crate::error::CodecError;
use crate::id::{encode_id_literal, encode_response_id_literal};
use crate::message::Message;

/// NATS wire representation produced by [`encode`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Encoded {
    pub headers: HeaderMap,
    pub body: Bytes,
}

/// Map a canonical JSON-RPC message onto NATS headers and body.
pub fn encode(message: &Message) -> Result<Encoded, CodecError> {
    let mut headers = HeaderMap::new();
    let body = match message {
        Message::Request { id, params, .. } => {
            headers.insert(HEADER_ID, encode_id_literal(id));
            serde_json::to_vec(params).map_err(CodecError::Serialize)?
        }
        Message::Notification { params, .. } => serde_json::to_vec(params).map_err(CodecError::Serialize)?,
        Message::Success { id, result } => {
            if let Some(literal) = encode_response_id_literal(id) {
                headers.insert(HEADER_ID, literal);
            }
            serde_json::to_vec(result).map_err(CodecError::Serialize)?
        }
        Message::Error {
            id,
            code,
            message: msg,
            data,
        } => {
            if let Some(literal) = encode_response_id_literal(id) {
                headers.insert(HEADER_ID, literal);
            }
            headers.insert(HEADER_ERROR_CODE, code.to_string());
            let mut body_map = serde_json::Map::new();
            body_map.insert("message".to_string(), serde_json::Value::String(msg.clone()));
            if let Some(data) = data {
                body_map.insert("data".to_string(), data.clone());
            }
            serde_json::to_vec(&serde_json::Value::Object(body_map)).map_err(CodecError::Serialize)?
        }
    };

    Ok(Encoded {
        headers,
        body: Bytes::from(body),
    })
}
