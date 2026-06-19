use tracing::{instrument, warn};

use crate::jsonrpc::extract_request_id;
use crate::server::handler::{A2aError, A2aExecutor};
use crate::server::wire::{JsonRpcErrorResponse, JsonRpcResponse, is_notification, parse_request};

#[instrument(name = "a2a.server.tasks_get", skip(handler, payload, reply_subject, nats))]
pub async fn handle<H, N>(handler: &H, payload: &[u8], reply_subject: Option<String>, nats: &N)
where
    H: A2aExecutor,
    N: trogon_nats::PublishClient,
{
    let Some(reply) = reply_subject else {
        warn!("tasks/get received without reply subject; dropping");
        return;
    };

    let id = extract_request_id(payload);
    if id.is_none() && is_notification(payload) {
        return;
    }

    let result = match parse_request::<a2a::types::GetTaskRequest>(payload) {
        Ok(req) => match req.params {
            Some(params) => handler.tasks_get(params).await,
            None => Err(A2aError::new(-32602, "Invalid params: missing params")),
        },
        Err(_) => Err(A2aError::new(-32700, "Parse error")),
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
                warn!(error = %e, "failed to publish tasks/get reply");
            }
        }
        Err(e) => warn!(error = %e, "failed to serialize tasks/get response"),
    }
}

#[cfg(test)]
mod tests {
    use trogon_nats::AdvancedMockNatsClient;

    use super::*;
    use crate::server::test_support::{parse_response, stub};

    fn get_task_payload(id: i64, task_id: &str) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tasks/get",
            "params": { "id": task_id }
        }))
        .unwrap()
    }

    fn task(task_id: &str) -> a2a::types::Task {
        a2a::types::Task {
            id: task_id.to_string(),
            context_id: String::new(),
            status: a2a::types::TaskStatus {
                state: a2a::types::TaskState::Working,
                message: None,
                timestamp: None,
            },
            artifacts: None,
            history: None,
            metadata: None,
        }
    }

    #[tokio::test]
    async fn success_publishes_task() {
        let nats = AdvancedMockNatsClient::new();
        let handler = stub();
        handler.lock().unwrap().tasks_get_result = Some(Ok(task("t-1")));
        handle(&handler, &get_task_payload(1, "t-1"), Some("r".into()), &nats).await;
        let body = parse_response(&nats.published_payloads()[0]);
        assert_eq!(body["result"]["id"].as_str(), Some("t-1"));
    }

    #[tokio::test]
    async fn task_not_found_error_uses_typed_code() {
        let nats = AdvancedMockNatsClient::new();
        let handler = stub();
        handler.lock().unwrap().tasks_get_result = Some(Err(A2aError::task_not_found("missing")));
        handle(&handler, &get_task_payload(2, "t"), Some("r".into()), &nats).await;
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
        handle(&handler, &get_task_payload(3, "t"), None, &nats).await;
        assert!(nats.published_messages().is_empty());
    }

    #[tokio::test]
    async fn missing_params_returns_invalid_params_error() {
        let nats = AdvancedMockNatsClient::new();
        let handler = stub();
        let payload = serde_json::to_vec(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 5,
            "method": "tasks/get"
        }))
        .unwrap();
        handle(&handler, &payload, Some("r".into()), &nats).await;
        let body = parse_response(&nats.published_payloads()[0]);
        assert_eq!(body["error"]["code"], -32602);
    }

    #[tokio::test]
    async fn parse_error_returns_jsonrpc_parse_error_code() {
        let nats = AdvancedMockNatsClient::new();
        let handler = stub();
        let payload = serde_json::to_vec(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 6,
            "method": "tasks/get",
            "params": { "id": 42 }
        }))
        .unwrap();
        handle(&handler, &payload, Some("r".into()), &nats).await;
        let body = parse_response(&nats.published_payloads()[0]);
        assert_eq!(body["error"]["code"], -32700);
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
            "method": "tasks/get",
            "params": {"id": "t"}
        }))
        .unwrap();
        handle(&handler, &payload, Some("r".into()), &nats).await;
        assert!(nats.published_messages().is_empty());
    }
}
