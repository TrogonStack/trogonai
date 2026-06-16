//! Thin serde wrappers for the JSON-RPC 2.0 framing used by A2A over NATS.
//!
//! `a2a-types` generates prost+pbjson structs for the inner message/task types but does
//! not define JSON-RPC request/response envelopes. These thin types do. We embed the
//! inner params/result as `serde_json::Value` or typed generics so we can parse the
//! envelope cheaply and delegate inner deserialization to the typed A2A structs.

use crate::jsonrpc::JsonRpcId;

/// Inbound JSON-RPC 2.0 request from a client.
#[derive(Debug, serde::Deserialize)]
pub struct JsonRpcRequest<P> {
    #[serde(rename = "jsonrpc")]
    #[allow(dead_code)]
    pub version: String,
    pub id: Option<JsonRpcId>,
    #[allow(dead_code)]
    pub method: Option<String>,
    pub params: Option<P>,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_valid_request() {
        let raw = br#"{"jsonrpc":"2.0","id":1,"method":"message/send","params":{"key":"val"}}"#;
        let req: JsonRpcRequest<serde_json::Value> = parse_request(raw).unwrap();
        assert_eq!(req.version, "2.0");
        assert_eq!(req.id, Some(JsonRpcId::Number(1)));
        assert_eq!(req.method.as_deref(), Some("message/send"));
    }

    #[test]
    fn parse_request_missing_params() {
        let raw = br#"{"jsonrpc":"2.0","id":1,"method":"tasks/get"}"#;
        let req: JsonRpcRequest<serde_json::Value> = parse_request(raw).unwrap();
        assert!(req.params.is_none());
    }

    #[test]
    fn parse_request_malformed() {
        let raw = b"not json";
        let result: Result<JsonRpcRequest<serde_json::Value>, _> = parse_request(raw);
        assert!(result.is_err());
    }

    #[test]
    fn response_serializes_with_2_0_version() {
        let resp = JsonRpcResponse::new(Some(JsonRpcId::Number(1)), serde_json::json!({"ok": true}));
        let bytes = resp.to_bytes().unwrap();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["jsonrpc"], "2.0");
        assert_eq!(v["id"], 1);
        assert_eq!(v["result"]["ok"], true);
    }

    #[test]
    fn error_response_round_trips() {
        let resp = JsonRpcErrorResponse::new(Some(JsonRpcId::String("x".into())), -32001, "not found");
        let bytes = resp.to_bytes().unwrap();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["jsonrpc"], "2.0");
        assert_eq!(v["error"]["code"], -32001);
        assert_eq!(v["error"]["message"], "not found");
        assert_eq!(v["id"], "x");
    }

    #[test]
    fn parse_error_response_has_32700() {
        let resp = JsonRpcErrorResponse::parse_error(None);
        assert_eq!(resp.error.code, -32700);
    }

    #[test]
    fn internal_error_response() {
        let resp = JsonRpcErrorResponse::internal_error(None, "boom");
        assert_eq!(resp.error.code, -32603);
        assert_eq!(resp.error.message, "boom");
    }
}
