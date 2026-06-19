use tracing::{instrument, warn};

use crate::jsonrpc::extract_request_id;
use crate::server::handler::{A2aError, A2aExecutor};
use crate::server::wire::{JsonRpcErrorResponse, JsonRpcResponse, is_notification, parse_request};

#[instrument(name = "a2a.server.push_delete", skip(handler, payload, reply_subject, nats))]
pub async fn handle<H, N>(handler: &H, payload: &[u8], reply_subject: Option<String>, nats: &N)
where
    H: A2aExecutor,
    N: trogon_nats::PublishClient,
{
    let Some(reply) = reply_subject else {
        warn!("tasks/pushNotificationConfig/delete received without reply subject; dropping");
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
            Some(raw) => match serde_json::from_value::<a2a::types::DeleteTaskPushNotificationConfigRequest>(raw) {
                Err(e) => Err(A2aError::new(-32602, format!("Invalid params: {e}"))),
                Ok(params) => handler.push_notification_delete(params).await,
            },
        },
    };
    let bytes = match result {
        Ok(()) => JsonRpcResponse::new(id, serde_json::Value::Null).to_bytes(),
        Err(e) => JsonRpcErrorResponse::new(id, e.code, e.message).to_bytes(),
    };
    match bytes {
        Ok(b) => {
            let headers = async_nats::HeaderMap::new();
            if let Err(e) = nats
                .publish_with_headers(async_nats::Subject::from(reply.as_str()), headers, b)
                .await
            {
                warn!(error = %e, "failed to publish push_delete reply");
            }
        }
        Err(e) => warn!(error = %e, "failed to serialize push_delete response"),
    }
}

#[cfg(test)]
mod tests {
    use trogon_nats::AdvancedMockNatsClient;

    use super::*;
    use crate::server::test_support::{parse_response, stub};

    fn delete_request(id: &str) -> a2a::types::DeleteTaskPushNotificationConfigRequest {
        a2a::types::DeleteTaskPushNotificationConfigRequest {
            task_id: "task-1".to_string(),
            id: id.to_string(),
            tenant: None,
        }
    }

    fn delete_payload(req_id: i64, cfg_id: &str) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": req_id,
            "method": "tasks/pushNotificationConfig/delete",
            "params": delete_request(cfg_id)
        }))
        .unwrap()
    }

    #[tokio::test]
    async fn success_publishes_null_result() {
        let nats = AdvancedMockNatsClient::new();
        let handler = stub();
        handler.lock().unwrap().push_delete_result = Some(Ok(()));
        handle(&handler, &delete_payload(1, "c-1"), Some("r".into()), &nats).await;
        let body = parse_response(&nats.published_payloads()[0]);
        assert!(body["result"].is_null());
        assert!(body["error"].is_null());
    }

    #[tokio::test]
    async fn task_not_found_error_uses_typed_code() {
        let nats = AdvancedMockNatsClient::new();
        let handler = stub();
        handler.lock().unwrap().push_delete_result = Some(Err(A2aError::task_not_found("missing")));
        handle(&handler, &delete_payload(2, "c"), Some("r".into()), &nats).await;
        let body = parse_response(&nats.published_payloads()[0]);
        assert_eq!(
            body["error"]["code"].as_i64(),
            Some(i64::from(crate::error::TASK_NOT_FOUND))
        );
    }

    #[tokio::test]
    async fn no_reply_drops_request() {
        let nats = AdvancedMockNatsClient::new();
        let handler = stub();
        handle(&handler, &delete_payload(3, "c"), None, &nats).await;
        assert!(nats.published_messages().is_empty());
    }

    #[tokio::test]
    async fn missing_params_returns_invalid_params_error() {
        let nats = AdvancedMockNatsClient::new();
        let handler = stub();
        let payload = serde_json::to_vec(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 5,
            "method": "tasks/pushNotificationConfig/delete"
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
            "method": "tasks/pushNotificationConfig/delete",
            "params": { "taskId": 42 }
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
            "method": "tasks/pushNotificationConfig/delete",
            "params": delete_request("c")
        }))
        .unwrap();
        handle(&handler, &payload, Some("r".into()), &nats).await;
        assert!(nats.published_messages().is_empty());
    }
}
