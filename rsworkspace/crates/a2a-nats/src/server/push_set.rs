use tracing::{instrument, warn};

use crate::jsonrpc::extract_request_id;
use crate::server::handler::{A2aError, A2aExecutor};
use crate::server::wire::{JsonRpcErrorResponse, JsonRpcResponse, is_notification, parse_request};

/// Handles `tasks/pushNotificationConfig/set`.
///
/// Delivery-semantics extension wiring (registry tracking, `deliverySemantics`
/// extension field) lands in its own follow-up so this slice stays on the
/// minimal unary template the other per-op handlers use.
#[instrument(name = "a2a.server.push_set", skip(handler, payload, reply_subject, nats))]
pub async fn handle<H, N>(handler: &H, payload: &[u8], reply_subject: Option<String>, nats: &N)
where
    H: A2aExecutor,
    N: trogon_nats::PublishClient,
{
    let Some(reply) = reply_subject else {
        warn!("tasks/pushNotificationConfig/set received without reply subject; dropping");
        return;
    };

    let id = extract_request_id(payload);
    if id.is_none() && is_notification(payload) {
        return;
    }

    // Parse the envelope first as a generic Value so we can distinguish
    // malformed JSON (-32700) from a well-formed envelope with an invalid
    // params shape (-32602).
    let result = match parse_request::<serde_json::Value>(payload) {
        Err(_) => Err(A2aError::new(-32700, "Parse error")),
        Ok(envelope) => match envelope.params {
            None => Err(A2aError::new(-32602, "Invalid params: missing params")),
            Some(raw) => match serde_json::from_value::<a2a::types::TaskPushNotificationConfig>(raw) {
                Err(e) => Err(A2aError::new(-32602, format!("Invalid params: {e}"))),
                Ok(params) => handler.push_notification_set(params).await,
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
                warn!(error = %e, "failed to publish push_set reply");
            }
        }
        Err(e) => warn!(error = %e, "failed to serialize push_set response"),
    }
}

#[cfg(test)]
mod tests {
    use trogon_nats::AdvancedMockNatsClient;

    use super::*;
    use crate::server::test_support::{parse_response, stub};

    fn config(id: &str) -> a2a::types::TaskPushNotificationConfig {
        a2a::types::TaskPushNotificationConfig {
            url: "https://example.com/webhook".to_string(),
            id: Some(id.to_string()),
            task_id: "task-1".to_string(),
            token: None,
            authentication: None,
            tenant: None,
        }
    }

    fn set_payload(req_id: i64, cfg_id: &str) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": req_id,
            "method": "tasks/pushNotificationConfig/set",
            "params": config(cfg_id)
        }))
        .unwrap()
    }

    #[tokio::test]
    async fn success_publishes_config() {
        let nats = AdvancedMockNatsClient::new();
        let handler = stub();
        handler.lock().unwrap().push_set_result = Some(Ok(config("c-1")));
        handle(&handler, &set_payload(1, "c-1"), Some("r".into()), &nats).await;
        let body = parse_response(&nats.published_payloads()[0]);
        assert_eq!(body["result"]["id"].as_str(), Some("c-1"));
    }

    #[tokio::test]
    async fn push_not_supported_error_uses_typed_code() {
        let nats = AdvancedMockNatsClient::new();
        let handler = stub();
        handler.lock().unwrap().push_set_result = Some(Err(A2aError::push_notification_not_supported("no push")));
        handle(&handler, &set_payload(2, "c"), Some("r".into()), &nats).await;
        let body = parse_response(&nats.published_payloads()[0]);
        assert_eq!(
            body["error"]["code"].as_i64(),
            Some(i64::from(crate::error::PUSH_NOTIFICATION_NOT_SUPPORTED))
        );
    }

    #[tokio::test]
    async fn no_reply_drops_request() {
        let nats = AdvancedMockNatsClient::new();
        let handler = stub();
        handle(&handler, &set_payload(3, "c"), None, &nats).await;
        assert!(nats.published_messages().is_empty());
    }

    #[tokio::test]
    async fn missing_params_returns_invalid_params_error() {
        let nats = AdvancedMockNatsClient::new();
        let handler = stub();
        let payload = serde_json::to_vec(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 5,
            "method": "tasks/pushNotificationConfig/set"
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
            "method": "tasks/pushNotificationConfig/set",
            "params": { "url": 42 }
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
            "method": "tasks/pushNotificationConfig/set",
            "params": config("c")
        }))
        .unwrap();
        handle(&handler, &payload, Some("r".into()), &nats).await;
        assert!(nats.published_messages().is_empty());
    }
}
