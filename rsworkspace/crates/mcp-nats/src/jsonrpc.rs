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
mod tests {
    use super::*;
    use rmcp::model::{JsonObject, Request};

    #[test]
    fn uses_rmcp_json_rpc_message_types() {
        let mut request = Request::new(JsonObject::new());
        request.method = "tools/list".to_string();
        let message = McpJsonRpcMessage::request(request, RequestId::Number(1));
        let serialized = serde_json::to_value(message).unwrap();

        assert_eq!(serialized["jsonrpc"], "2.0");
        assert_eq!(serialized["id"], 1);
        assert_eq!(serialized["method"], "tools/list");
    }

    #[test]
    fn extracts_sdk_request_id_without_params_type() {
        let payload = br#"{"jsonrpc":"2.0","id":"abc","method":"tools/list","params":{"x":1}}"#;
        assert_eq!(extract_request_id(payload), Some(RequestId::String("abc".into())));
    }

    #[test]
    fn extract_request_id_returns_none_for_notifications() {
        let payload = br#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
        assert_eq!(extract_request_id(payload), None);
    }
}
