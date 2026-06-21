use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RpcId {
    Number(i64),
    String(String),
    Null,
}

#[derive(Debug, Deserialize)]
pub struct InboundRequest {
    pub id: RpcId,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

#[derive(Debug, Serialize)]
pub struct OutboundResponse {
    pub jsonrpc: &'static str,
    pub id: RpcId,
    pub result: Value,
}

impl OutboundResponse {
    pub fn new(id: RpcId, result: Value) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result,
        }
    }
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

#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum OutboundFrame {
    Response(OutboundResponse),
    Notification(OutboundNotification),
    Error(OutboundError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn inbound_request_deserializes_numeric_id() {
        let raw = r#"{"jsonrpc":"2.0","id":42,"method":"tasks/get","params":{"id":"t1","tenant":""}}"#;
        let req: InboundRequest = serde_json::from_str(raw).unwrap();
        assert_eq!(req.id, RpcId::Number(42));
        assert_eq!(req.method, "tasks/get");
    }

    #[test]
    fn inbound_request_deserializes_string_id() {
        let raw = r#"{"jsonrpc":"2.0","id":"abc","method":"agent/getAuthenticatedExtendedCard","params":{}}"#;
        let req: InboundRequest = serde_json::from_str(raw).unwrap();
        assert_eq!(req.id, RpcId::String("abc".into()));
    }

    #[test]
    fn outbound_response_serializes() {
        let resp = OutboundResponse::new(RpcId::Number(1), json!({"id": "task-1"}));
        let v = serde_json::to_value(&resp).unwrap();
        assert_eq!(v["jsonrpc"], "2.0");
        assert_eq!(v["id"], 1);
        assert!(v["result"]["id"] == "task-1");
    }

    #[test]
    fn outbound_error_serializes() {
        let err = OutboundError::new(RpcId::Number(2), -32001, "not found".into());
        let v = serde_json::to_value(&err).unwrap();
        assert_eq!(v["error"]["code"], -32001);
        assert_eq!(v["error"]["message"], "not found");
    }

    #[test]
    fn outbound_notification_serializes() {
        let notif = OutboundNotification::new(RpcId::Number(3), "message/stream", json!({"event": "x"}));
        let v = serde_json::to_value(&notif).unwrap();
        assert_eq!(v["method"], "message/stream");
        assert_eq!(v["id"], 3);
    }

    #[test]
    fn outbound_frame_error_variant_serializes() {
        let frame = OutboundFrame::Error(OutboundError::new(RpcId::Null, -32600, "invalid".into()));
        let v = serde_json::to_value(&frame).unwrap();
        assert_eq!(v["error"]["code"], -32600);
    }
}
