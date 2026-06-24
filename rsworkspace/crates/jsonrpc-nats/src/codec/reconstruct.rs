use crate::constants::JSONRPC_VERSION;
use crate::error::CodecError;
use crate::id::{RequestId, ResponseId};
use crate::message::Message;
use serde::de::Error as _;
use serde_json::{Map, Value};

/// Reconstruct canonical JSON-RPC 2.0 from a codec message (for protocol edges).
pub fn to_json_value(message: &Message) -> Value {
    match message {
        Message::Request { id, method, params } => {
            let mut obj = Map::new();
            obj.insert("jsonrpc".to_string(), Value::String(JSONRPC_VERSION.to_string()));
            obj.insert("id".to_string(), serde_json::to_value(id).expect("id serializes"));
            obj.insert("method".to_string(), Value::String(method.clone()));
            obj.insert("params".to_string(), params.clone());
            Value::Object(obj)
        }
        Message::Notification { method, params } => {
            let mut obj = Map::new();
            obj.insert("jsonrpc".to_string(), Value::String(JSONRPC_VERSION.to_string()));
            obj.insert("method".to_string(), Value::String(method.clone()));
            obj.insert("params".to_string(), params.clone());
            Value::Object(obj)
        }
        Message::Success { id, result } => {
            let mut obj = Map::new();
            obj.insert("jsonrpc".to_string(), Value::String(JSONRPC_VERSION.to_string()));
            obj.insert("id".to_string(), serde_json::to_value(id).expect("id serializes"));
            obj.insert("result".to_string(), result.clone());
            Value::Object(obj)
        }
        Message::Error {
            id,
            code,
            message,
            data,
        } => {
            let mut error = Map::new();
            error.insert("code".to_string(), Value::Number((*code).into()));
            error.insert("message".to_string(), Value::String(message.clone()));
            if let Some(data) = data {
                error.insert("data".to_string(), data.clone());
            }
            let mut obj = Map::new();
            obj.insert("jsonrpc".to_string(), Value::String(JSONRPC_VERSION.to_string()));
            obj.insert("id".to_string(), serde_json::to_value(id).expect("id serializes"));
            obj.insert("error".to_string(), Value::Object(error));
            Value::Object(obj)
        }
    }
}

/// Parse a canonical JSON-RPC 2.0 value into a codec message.
pub fn from_json_value(value: &Value) -> Result<Message, CodecError> {
    let obj = value.as_object().ok_or_else(|| {
        CodecError::Deserialize(serde_json::Error::custom("expected JSON-RPC object"))
    })?;

    if obj.get("jsonrpc").and_then(Value::as_str) != Some(JSONRPC_VERSION) {
        return Err(CodecError::Deserialize(serde_json::Error::custom(
            "unsupported jsonrpc version",
        )));
    }

    if let Some(method) = obj.get("method").and_then(Value::as_str) {
        let params = obj
            .get("params")
            .cloned()
            .unwrap_or(Value::Null);
        return match obj.get("id") {
            Some(id_value) => {
                if id_value.is_null() {
                    return Err(CodecError::RequestWithoutId);
                }
                let id: RequestId = serde_json::from_value(id_value.clone()).map_err(CodecError::Deserialize)?;
                Ok(Message::Request {
                    id,
                    method: method.to_string(),
                    params,
                })
            }
            None => Ok(Message::Notification {
                method: method.to_string(),
                params,
            }),
        };
    }

    let id: ResponseId = match obj.get("id") {
        Some(id_value) => serde_json::from_value(id_value.clone()).map_err(CodecError::Deserialize)?,
        None => ResponseId::Null,
    };

    if let Some(error) = obj.get("error") {
        let code = error
            .get("code")
            .and_then(Value::as_i64)
            .ok_or_else(|| CodecError::Deserialize(serde_json::Error::custom("error missing code")))?
            as i32;
        let message = error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let data = error.get("data").cloned();
        return Ok(Message::Error {
            id,
            code,
            message,
            data,
        });
    }

    let result = obj
        .get("result")
        .cloned()
        .ok_or(CodecError::AmbiguousResponse)?;
    Ok(Message::Success { id, result })
}
