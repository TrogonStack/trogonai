pub use rmcp::ErrorData;
pub use rmcp::model::{
    ClientJsonRpcMessage, JsonRpcError, JsonRpcMessage, JsonRpcNotification, JsonRpcRequest, JsonRpcResponse,
    RequestId, ServerJsonRpcMessage,
};

pub type McpJsonRpcMessage = JsonRpcMessage;
pub type McpJsonRpcRequest = JsonRpcRequest;
pub type McpJsonRpcNotification = JsonRpcNotification;
pub type McpJsonRpcResponse = JsonRpcResponse;
pub type McpJsonRpcError = JsonRpcError;

pub fn extract_request_id(payload: &[u8]) -> Option<RequestId> {
    let value = serde_json::from_slice::<serde_json::Value>(payload).ok()?;
    let id = value.get("id")?;
    serde_json::from_value::<RequestId>(id.clone()).ok()
}

#[cfg(test)]
mod tests;
