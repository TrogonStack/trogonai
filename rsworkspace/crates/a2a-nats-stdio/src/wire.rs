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
mod tests;
