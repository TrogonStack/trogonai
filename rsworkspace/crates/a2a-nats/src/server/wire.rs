//! Thin serde wrappers for the JSON-RPC 2.0 framing used by A2A over NATS.
//!
//! `a2a-types` generates prost+pbjson structs for the inner message/task types but does
//! not define JSON-RPC request/response envelopes. These thin types do. We embed the
//! inner params/result as `serde_json::Value` or typed generics so we can parse the
//! envelope cheaply and delegate inner deserialization to the typed A2A structs.

use serde::Deserialize as _;
use serde::de::IntoDeserializer;

use crate::jsonrpc::JsonRpcId;

/// Inbound JSON-RPC 2.0 request from a client.
///
/// `id` distinguishes three wire states the JSON-RPC 2.0 spec treats as
/// semantically different:
/// - field absent → notification (no response expected) → `None`
/// - field present with explicit `null` → request with null id → `Some(JsonRpcId::Null)`
/// - field present with number/string → typed id → `Some(JsonRpcId::{Number,String})`
#[derive(Debug, serde::Deserialize)]
pub struct JsonRpcRequest<P> {
    #[serde(rename = "jsonrpc")]
    #[allow(dead_code)]
    pub version: String,
    #[serde(default, deserialize_with = "deserialize_optional_id")]
    pub id: Option<JsonRpcId>,
    #[allow(dead_code)]
    pub method: Option<String>,
    pub params: Option<P>,
}

fn deserialize_optional_id<'de, D>(deserializer: D) -> Result<Option<JsonRpcId>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value: serde_json::Value = serde::Deserialize::deserialize(deserializer)?;
    if value.is_null() {
        return Ok(Some(JsonRpcId::Null));
    }
    JsonRpcId::deserialize(value.into_deserializer())
        .map(Some)
        .map_err(<D::Error as serde::de::Error>::custom)
}

/// Outbound JSON-RPC 2.0 success response.
#[derive(Debug, serde::Serialize)]
pub struct JsonRpcResponse<R> {
    pub jsonrpc: &'static str,
    pub id: Option<JsonRpcId>,
    pub result: R,
}

/// Outbound JSON-RPC 2.0 error response.
#[derive(Debug, serde::Serialize)]
pub struct JsonRpcErrorResponse {
    pub jsonrpc: &'static str,
    pub id: Option<JsonRpcId>,
    pub error: JsonRpcErrorBody,
}

#[derive(Debug, serde::Serialize)]
pub struct JsonRpcErrorBody {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

impl<R: serde::Serialize> JsonRpcResponse<R> {
    pub fn new(id: Option<JsonRpcId>, result: R) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result,
        }
    }

    pub fn to_bytes(&self) -> Result<bytes::Bytes, serde_json::Error> {
        serde_json::to_vec(self).map(bytes::Bytes::from)
    }
}

impl JsonRpcErrorResponse {
    pub fn new(id: Option<JsonRpcId>, code: i32, message: impl Into<String>) -> Self {
        Self::with_data(id, code, message, None)
    }

    pub fn with_data(
        id: Option<JsonRpcId>,
        code: i32,
        message: impl Into<String>,
        data: Option<serde_json::Value>,
    ) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            error: JsonRpcErrorBody {
                code,
                message: message.into(),
                data,
            },
        }
    }

    #[allow(dead_code)]
    pub fn parse_error(id: Option<JsonRpcId>) -> Self {
        Self::new(id, -32700, "Parse error")
    }

    #[allow(dead_code)]
    pub fn internal_error(id: Option<JsonRpcId>, message: impl Into<String>) -> Self {
        Self::new(id, -32603, message)
    }

    pub fn to_bytes(&self) -> Result<bytes::Bytes, serde_json::Error> {
        serde_json::to_vec(self).map(bytes::Bytes::from)
    }
}

/// Parse a raw NATS payload as a typed JSON-RPC request.
pub fn parse_request<P: serde::de::DeserializeOwned>(raw: &[u8]) -> Result<JsonRpcRequest<P>, serde_json::Error> {
    serde_json::from_slice(raw)
}

/// True only when the payload is a parseable JSON object that omits the `id`
/// key, matching JSON-RPC 2.0's definition of a notification. Anything else —
/// non-JSON, non-object, or an `id` value the extractor couldn't decode — is a
/// malformed request that still warrants an error reply so request/reply
/// clients don't hang.
pub fn is_notification(payload: &[u8]) -> bool {
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(payload) else {
        return false;
    };
    matches!(value, serde_json::Value::Object(ref map) if !map.contains_key("id"))
}

#[cfg(test)]
mod tests;
