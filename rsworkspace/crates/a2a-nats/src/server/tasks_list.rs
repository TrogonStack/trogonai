use tracing::{instrument, warn};

use crate::jsonrpc::extract_request_id;
use crate::server::handler::{A2aError, A2aExecutor};
use crate::server::wire::{JsonRpcErrorResponse, JsonRpcResponse, is_notification, parse_request};

#[instrument(name = "a2a.server.tasks_list", skip(handler, payload, reply_subject, nats))]
pub async fn handle<H, N>(handler: &H, payload: &[u8], reply_subject: Option<String>, nats: &N)
where
    H: A2aExecutor,
    N: trogon_nats::PublishClient,
{
    let Some(reply) = reply_subject else {
        warn!("tasks/list received without reply subject; dropping");
        return;
    };

    let id = extract_request_id(payload);
    if id.is_none() && is_notification(payload) {
        return;
    }

    let result = match parse_request::<serde_json::Value>(payload) {
        Err(_) => Err(A2aError::new(-32700, "Parse error")),
        Ok(envelope) => match envelope.params {
            None => Err(A2aError::new(-32602, "Invalid params: missing params")),
            Some(raw) => match serde_json::from_value::<a2a::types::ListTasksRequest>(raw) {
                Err(e) => Err(A2aError::new(-32602, format!("Invalid params: {e}"))),
                Ok(params) => handler.tasks_list(params).await,
            },
        },
    };
    let bytes = match result {
        Ok(resp) => JsonRpcResponse::new(id, resp).to_bytes(),
        Err(e) => JsonRpcErrorResponse::new(id, e.code, e.message).to_bytes(),
    };
    match bytes {
        Ok(b) => {
            let headers = async_nats::HeaderMap::new();
            if let Err(e) = nats
                .publish_with_headers(async_nats::Subject::from(reply.as_str()), headers, b)
                .await
            {
                warn!(error = %e, "failed to publish tasks/list reply");
            }
        }
        Err(e) => warn!(error = %e, "failed to serialize tasks/list response"),
    }
}

#[cfg(test)]
mod tests {
    use trogon_nats::AdvancedMockNatsClient;

    use super::*;
    use crate::server::test_support::{parse_response, stub};

    fn list_payload(id: i64) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tasks/list",
            "params": {}
        }))
        .unwrap()
    }

    fn empty_response() -> a2a::types::ListTasksResponse {
        a2a::types::ListTasksResponse {
            tasks: vec![],
            next_page_token: String::new(),
            page_size: 0,
            total_size: 0,
        }
    }

    #[tokio::test]
    async fn success_publishes_list_response() {
        let nats = AdvancedMockNatsClient::new();
        let handler = stub();
        handler.lock().unwrap().tasks_list_result = Some(Ok(empty_response()));
        handle(&handler, &list_payload(1), Some("r".into()), &nats).await;
        let body = parse_response(&nats.published_payloads()[0]);
        assert!(body["result"]["tasks"].is_array());
    }

    #[tokio::test]
    async fn handler_error_response_uses_typed_code() {
        let nats = AdvancedMockNatsClient::new();
        let handler = stub();
        handler.lock().unwrap().tasks_list_result = Some(Err(A2aError::agent_unavailable("down")));
        handle(&handler, &list_payload(2), Some("r".into()), &nats).await;
        let body = parse_response(&nats.published_payloads()[0]);
        assert_eq!(
            body["error"]["code"].as_i64(),
            Some(i64::from(crate::error::AGENT_UNAVAILABLE))
        );
    }

    #[tokio::test]
    async fn no_reply_drops_request() {
        let nats = AdvancedMockNatsClient::new();
        let handler = stub();
        handle(&handler, &list_payload(3), None, &nats).await;
        assert!(nats.published_messages().is_empty());
    }

    #[tokio::test]
    async fn missing_params_returns_invalid_params_error() {
        let nats = AdvancedMockNatsClient::new();
        let handler = stub();
        let payload = serde_json::to_vec(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 5,
            "method": "tasks/list"
        }))
        .unwrap();
        handle(&handler, &payload, Some("r".into()), &nats).await;
        let body = parse_response(&nats.published_payloads()[0]);
        assert_eq!(body["error"]["code"], -32602);
    }

    #[tokio::test]
    async fn invalid_params_shape_returns_invalid_params_code() {
        let nats = AdvancedMockNatsClient::new();
        let handler = stub();
        let payload = serde_json::to_vec(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 6,
            "method": "tasks/list",
            "params": { "pageSize": "not-a-number" }
        }))
        .unwrap();
        handle(&handler, &payload, Some("r".into()), &nats).await;
        let body = parse_response(&nats.published_payloads()[0]);
        assert_eq!(body["error"]["code"], -32602);
        assert_eq!(body["id"], 6);
    }

    #[tokio::test]
    async fn malformed_json_still_publishes_parse_error_with_null_id() {
        let nats = AdvancedMockNatsClient::new();
        let handler = stub();
        handle(&handler, b"not json", Some("r".into()), &nats).await;
        let body = parse_response(&nats.published_payloads()[0]);
        assert_eq!(body["error"]["code"], -32700);
        assert!(body["id"].is_null());
    }

    #[tokio::test]
    async fn notification_without_id_is_dropped() {
        let nats = AdvancedMockNatsClient::new();
        let handler = stub();
        let payload = serde_json::to_vec(&serde_json::json!({
            "jsonrpc": "2.0",
            "method": "tasks/list",
            "params": {}
        }))
        .unwrap();
        handle(&handler, &payload, Some("r".into()), &nats).await;
        assert!(nats.published_messages().is_empty());
    }
}
