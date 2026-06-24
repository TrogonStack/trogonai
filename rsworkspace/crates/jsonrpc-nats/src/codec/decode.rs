use async_nats::header::HeaderMap;

use crate::constants::{HEADER_ERROR_CODE, HEADER_ID};
use crate::direction::Direction;
use crate::error::CodecError;
use crate::id::{decode_request_id_literal, decode_response_id_literal};
use crate::message::Message;

/// Decode a NATS message into a canonical JSON-RPC message.
///
/// `method` must be supplied for [`Direction::Request`] (from the NATS subject).
pub fn decode(
    direction: Direction,
    method: Option<&str>,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Message, CodecError> {
    let id_header = headers.get(HEADER_ID).map(|values| values.as_str().to_string());
    let error_code_header = headers
        .get(HEADER_ERROR_CODE)
        .map(|values| values.as_str().to_string());

    match direction {
        Direction::Request => {
            let method = method.ok_or(CodecError::MissingMethod)?.to_string();
            match id_header {
                Some(value) => {
                    let id = decode_request_id_literal(&value)?;
                    let params = if body.is_empty() {
                        serde_json::Value::Null
                    } else {
                        serde_json::from_slice(body).map_err(CodecError::Deserialize)?
                    };
                    Ok(Message::Request { id, method, params })
                }
                None => {
                    if error_code_header.is_some() {
                        return Err(CodecError::AmbiguousResponse);
                    }
                    let params = if body.is_empty() {
                        serde_json::Value::Null
                    } else {
                        serde_json::from_slice(body).map_err(CodecError::Deserialize)?
                    };
                    Ok(Message::Notification { method, params })
                }
            }
        }
        Direction::Response => {
            let id = match id_header {
                Some(value) => decode_response_id_literal(&value)?,
                None => crate::id::ResponseId::Null,
            };

            if let Some(code_str) = error_code_header {
                let code: i32 = code_str.parse().map_err(|_| CodecError::InvalidErrorCodeHeader {
                    value: code_str,
                })?;
                let (message, data) = if body.is_empty() {
                    (String::new(), None)
                } else {
                    let value: serde_json::Value =
                        serde_json::from_slice(body).map_err(CodecError::Deserialize)?;
                    let obj = value.as_object().ok_or_else(|| {
                        CodecError::Deserialize(serde::de::Error::custom("error body must be object"))
                    })?;
                    let message = obj
                        .get("message")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    let data = obj.get("data").map(|v| v.clone());
                    (message, data)
                };
                return Ok(Message::Error {
                    id,
                    code,
                    message,
                    data,
                });
            }

            if body.is_empty() {
                return Err(CodecError::AmbiguousResponse);
            }

            let result = serde_json::from_slice(body).map_err(CodecError::Deserialize)?;
            Ok(Message::Success { id, result })
        }
    }
}
