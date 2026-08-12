use bytes::Bytes;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RpcId {
    Number(i64),
    String(String),
    Null,
}

impl RpcId {
    pub fn to_json_value(&self) -> Value {
        match self {
            Self::Number(n) => Value::Number((*n).into()),
            Self::String(s) => Value::String(s.clone()),
            Self::Null => Value::Null,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct InboundRequest {
    pub id: RpcId,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

#[derive(Debug, Serialize)]
pub struct OutboundNotification {
    pub jsonrpc: &'static str,
    pub id: RpcId,
    pub method: &'static str,
    pub params: Value,
}

impl OutboundNotification {
    pub fn new(id: RpcId, method: &'static str, params: Value) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            method,
            params,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct RpcError {
    pub code: i32,
    pub message: String,
}

#[derive(Debug, Serialize)]
pub struct OutboundError {
    pub jsonrpc: &'static str,
    pub id: RpcId,
    pub error: RpcError,
}

impl OutboundError {
    pub fn new(id: RpcId, code: i32, message: String) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            error: RpcError { code, message },
        }
    }
}

/// Stdio outbound frame. Success responses prefer a validated NATS body
/// (id rewritten to the edge client id); local errors still use a typed
/// envelope built via serde.
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum OutboundFrame {
    /// Canonical JSON-RPC body bytes after typed validate + id rewrite.
    #[serde(serialize_with = "serialize_raw_body")]
    RawBody(Bytes),
    Notification(OutboundNotification),
    Error(OutboundError),
}

fn serialize_raw_body<S>(body: &Bytes, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    let value: Value = serde_json::from_slice(body).map_err(serde::ser::Error::custom)?;
    value.serialize(serializer)
}

#[cfg(test)]
mod tests;
