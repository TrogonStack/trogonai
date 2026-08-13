use async_nats::header::HeaderMap;
use bytes::Bytes;
use serde_json::Value;

use crate::constants::{HEADER_ERROR_CODE, HEADER_ID};
use crate::direction::Direction;
use crate::error::CodecError;
use crate::id::{ResponseId, decode_response_id_literal, encode_id_literal, encode_response_id_literal};
use crate::message::Message;

use super::{Encoded, from_json_value, to_json_value};

/// Encode a complete canonical JSON-RPC 2.0 object in the NATS body.
///
/// The body is authoritative. `Jsonrpc-Id` and `Jsonrpc-Error-Code` are emitted
/// as non-authoritative projections of the body for cheap routing and metrics
/// (ADR#0056). Decoders reject a present header that disagrees with the body.
pub fn encode(message: &Message) -> Result<Encoded, CodecError> {
    let mut value = to_json_value(message);
    // `Message` cannot distinguish an absent `params` from an explicit null, so
    // `to_json_value` renders an omitted `params` as `"params": null`. Canonical
    // JSON-RPC omits absent params and `validate_params` rejects a null, so drop
    // it here to keep paramless requests and notifications round-trippable.
    if let Value::Object(object) = &mut value
        && object.get("params").is_some_and(Value::is_null)
    {
        object.remove("params");
    }
    let body = serde_json::to_vec(&value).map_err(CodecError::Serialize)?;
    Ok(Encoded {
        headers: derived_projection_headers(message),
        body: Bytes::from(body),
    })
}

/// Encode a complete canonical JSON-RPC 2.0 value without normalizing its shape.
///
/// The value is parsed for protocol validation, but the original value is the
/// one serialized. This preserves absent optional members and extensions.
/// Derived `Jsonrpc-*` headers are projected from the parsed message.
pub fn encode_value(value: &Value) -> Result<Encoded, CodecError> {
    let message = parse_canonical_value(value)?;
    let body = serde_json::to_vec(value).map_err(CodecError::Serialize)?;
    Ok(Encoded {
        headers: derived_projection_headers(&message),
        body: Bytes::from(body),
    })
}

/// Non-authoritative projections of the body for routing and metrics.
fn derived_projection_headers(message: &Message) -> HeaderMap {
    let mut headers = HeaderMap::new();
    match message {
        Message::Request { id, .. } => {
            headers.insert(HEADER_ID, encode_id_literal(id));
        }
        Message::Notification { .. } => {}
        Message::Success { id, .. } => {
            if let Some(literal) = encode_response_id_literal(id) {
                headers.insert(HEADER_ID, literal);
            }
        }
        Message::Error { id, code, .. } => {
            if let Some(literal) = encode_response_id_literal(id) {
                headers.insert(HEADER_ID, literal);
            }
            headers.insert(HEADER_ERROR_CODE, code.to_string());
        }
    }
    headers
}

/// Decode a canonical JSON-RPC 2.0 body and validate its NATS projections.
///
/// `projected_method` is required for request-direction messages because the
/// NATS subject mirrors the JSON-RPC method for routing. Derived `Jsonrpc-Id`
/// and `Jsonrpc-Error-Code` headers are optional, but when present they must
/// agree with the canonical body.
pub fn decode(
    direction: Direction,
    projected_method: Option<&str>,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Message, CodecError> {
    let (_, message) = decode_parts(direction, projected_method, headers, body)?;
    Ok(message)
}

/// Decode and validate a canonical JSON-RPC 2.0 body without normalizing it.
///
/// The returned value has exactly the member shape carried in the body.
pub fn decode_value(
    direction: Direction,
    projected_method: Option<&str>,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Value, CodecError> {
    let (value, _) = decode_parts(direction, projected_method, headers, body)?;
    Ok(value)
}

fn decode_parts(
    direction: Direction,
    projected_method: Option<&str>,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<(Value, Message), CodecError> {
    let value = serde_json::from_slice(body).map_err(CodecError::Deserialize)?;
    let message = parse_canonical_value(&value)?;

    validate_direction(direction, &message)?;
    validate_method_projection(direction, projected_method, &message)?;
    validate_id_projection(headers, &message)?;
    validate_error_code_projection(headers, &message)?;

    Ok((value, message))
}

fn parse_canonical_value(value: &Value) -> Result<Message, CodecError> {
    validate_canonical_envelope(value)?;
    from_json_value(value)
}

fn validate_canonical_envelope(value: &Value) -> Result<(), CodecError> {
    let Some(object) = value.as_object() else {
        return Ok(());
    };
    if object.get("jsonrpc").and_then(Value::as_str) != Some(crate::JSONRPC_VERSION) {
        return Ok(());
    }

    match object.get("method") {
        Some(method) => {
            if !method.is_string() {
                return Err(CodecError::InvalidCanonicalMethod);
            }
            if object.contains_key("result") || object.contains_key("error") {
                return Err(CodecError::CanonicalRequestWithResponseMembers);
            }
            validate_params(object.get("params"))
        }
        None => validate_response(object),
    }
}

fn validate_params(params: Option<&Value>) -> Result<(), CodecError> {
    match params {
        Some(Value::Object(_) | Value::Array(_)) | None => Ok(()),
        Some(_) => Err(CodecError::InvalidCanonicalParams),
    }
}

fn validate_response(object: &serde_json::Map<String, Value>) -> Result<(), CodecError> {
    if object.contains_key("params") {
        return Err(CodecError::CanonicalResponseWithRequestMembers);
    }
    if !object.contains_key("id") {
        return Err(CodecError::CanonicalResponseWithoutId);
    }

    match (object.get("result"), object.get("error")) {
        (Some(_), None) if object.get("id").is_some_and(Value::is_null) => Err(CodecError::CanonicalSuccessWithNullId),
        (Some(_), None) => Ok(()),
        (None, Some(error)) => validate_error(error),
        (Some(_), Some(_)) | (None, None) => Err(CodecError::InvalidCanonicalResponseShape),
    }
}

fn validate_error(error: &Value) -> Result<(), CodecError> {
    let object = error.as_object().ok_or(CodecError::InvalidCanonicalErrorObject)?;
    object
        .get("code")
        .and_then(Value::as_i64)
        .and_then(|code| i32::try_from(code).ok())
        .ok_or(CodecError::InvalidCanonicalErrorCode)?;
    if !object.get("message").is_some_and(Value::is_string) {
        return Err(CodecError::InvalidCanonicalErrorMessage);
    }
    Ok(())
}

fn validate_direction(direction: Direction, message: &Message) -> Result<(), CodecError> {
    let valid = matches!(
        (direction, message),
        (
            Direction::Request,
            Message::Request { .. } | Message::Notification { .. }
        ) | (Direction::Response, Message::Success { .. } | Message::Error { .. })
    );
    if valid {
        Ok(())
    } else {
        Err(CodecError::DirectionMismatch {
            expected: match direction {
                Direction::Request => "request or notification",
                Direction::Response => "success or error response",
            },
            actual: message_kind(message),
        })
    }
}

fn validate_method_projection(
    direction: Direction,
    projected_method: Option<&str>,
    message: &Message,
) -> Result<(), CodecError> {
    match direction {
        Direction::Request => {
            let projected = projected_method.ok_or(CodecError::MissingMethodProjection)?;
            let actual = message.method().ok_or(CodecError::MissingMethod)?;
            if projected == actual {
                Ok(())
            } else {
                Err(CodecError::MethodProjectionMismatch {
                    projected: projected.to_string(),
                    actual: actual.to_string(),
                })
            }
        }
        Direction::Response => match projected_method {
            Some(projected) => Err(CodecError::UnexpectedMethodProjection {
                projected: projected.to_string(),
            }),
            None => Ok(()),
        },
    }
}

fn validate_id_projection(headers: &HeaderMap, message: &Message) -> Result<(), CodecError> {
    let Some(value) = headers.get(HEADER_ID).map(|value| value.as_str()) else {
        return Ok(());
    };
    if matches!(message, Message::Notification { .. }) {
        return Err(CodecError::NotificationWithId);
    }

    let projected = decode_response_id_literal(value)?;
    let actual = match message {
        Message::Request { id, .. } => ResponseId::from(id.clone()),
        Message::Success { id, .. } | Message::Error { id, .. } => id.clone(),
        Message::Notification { .. } => return Err(CodecError::NotificationWithId),
    };
    if projected == actual {
        Ok(())
    } else {
        Err(CodecError::IdProjectionMismatch {
            projected: projected.to_json().to_string(),
            actual: actual.to_json().to_string(),
        })
    }
}

fn validate_error_code_projection(headers: &HeaderMap, message: &Message) -> Result<(), CodecError> {
    let Some(value) = headers.get(HEADER_ERROR_CODE).map(|value| value.as_str()) else {
        return Ok(());
    };
    let projected = value.parse::<i32>().map_err(|_| CodecError::InvalidErrorCodeHeader {
        value: value.to_string(),
    })?;
    match message {
        Message::Error { code, .. } if projected == *code => Ok(()),
        Message::Error { code, .. } => Err(CodecError::ErrorCodeProjectionMismatch {
            projected,
            actual: *code,
        }),
        _ => Err(CodecError::UnexpectedErrorCodeProjection { projected }),
    }
}

fn message_kind(message: &Message) -> &'static str {
    match message {
        Message::Request { .. } => "request",
        Message::Notification { .. } => "notification",
        Message::Success { .. } => "success response",
        Message::Error { .. } => "error response",
    }
}
