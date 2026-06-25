use crate::error::CodecError;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Non-null JSON-RPC request id.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RequestId {
    Number(i64),
    String(String),
}

/// JSON-RPC response id, including `null`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ResponseId {
    Number(i64),
    String(String),
    Null,
}

impl RequestId {
    /// Canonical JSON value for this id (infallible: ids are numbers or strings).
    pub fn to_json(&self) -> Value {
        match self {
            Self::Number(n) => Value::Number((*n).into()),
            Self::String(s) => Value::String(s.clone()),
        }
    }
}

impl ResponseId {
    /// Canonical JSON value for this id (infallible).
    pub fn to_json(&self) -> Value {
        match self {
            Self::Number(n) => Value::Number((*n).into()),
            Self::String(s) => Value::String(s.clone()),
            Self::Null => Value::Null,
        }
    }
}

impl From<RequestId> for ResponseId {
    fn from(id: RequestId) -> Self {
        match id {
            RequestId::Number(n) => Self::Number(n),
            RequestId::String(s) => Self::String(s),
        }
    }
}

impl TryFrom<ResponseId> for RequestId {
    type Error = CodecError;

    fn try_from(id: ResponseId) -> Result<Self, Self::Error> {
        match id {
            ResponseId::Number(n) => Ok(Self::Number(n)),
            ResponseId::String(s) => Ok(Self::String(s)),
            ResponseId::Null => Err(CodecError::RequestWithoutId),
        }
    }
}

/// Encode a non-null id as its JSON literal for the `Jsonrpc-Id` header.
pub fn encode_id_literal(id: &RequestId) -> String {
    id.to_json().to_string()
}

/// Encode a response id as its JSON literal; `null` omits the header.
pub fn encode_response_id_literal(id: &ResponseId) -> Option<String> {
    match id {
        ResponseId::Null => None,
        ResponseId::Number(n) => Some(n.to_string()),
        ResponseId::String(s) => Some(Value::String(s.clone()).to_string()),
    }
}

/// Decode a `Jsonrpc-Id` header value into a response id.
pub fn decode_response_id_literal(value: &str) -> Result<ResponseId, CodecError> {
    let parsed: Value = serde_json::from_str(value).map_err(|_| CodecError::InvalidIdHeader {
        value: value.to_string(),
    })?;
    decode_response_id_value(&parsed).ok_or_else(|| CodecError::InvalidIdHeader {
        value: value.to_string(),
    })
}

/// Decode a `Jsonrpc-Id` header value into a request id (non-null).
pub fn decode_request_id_literal(value: &str) -> Result<RequestId, CodecError> {
    let parsed: Value = serde_json::from_str(value).map_err(|_| CodecError::InvalidIdHeader {
        value: value.to_string(),
    })?;
    match parsed {
        Value::Number(n) => n
            .as_i64()
            .map(RequestId::Number)
            .ok_or_else(|| CodecError::InvalidIdHeader {
                value: value.to_string(),
            }),
        Value::String(s) => Ok(RequestId::String(s)),
        _ => Err(CodecError::InvalidIdHeader {
            value: value.to_string(),
        }),
    }
}

fn decode_response_id_value(value: &Value) -> Option<ResponseId> {
    match value {
        Value::Number(n) => n.as_i64().map(ResponseId::Number),
        Value::String(s) => Some(ResponseId::String(s.clone())),
        Value::Null => Some(ResponseId::Null),
        _ => None,
    }
}
