use tracing::{instrument, warn};

use crate::server::handler::{A2aError, A2aHandler};
use crate::server::wire::{JsonRpcErrorResponse, JsonRpcResponse, parse_request};
use crate::jsonrpc::JsonRpcId;

#[instrument(name = "a2a.agent.message_send", skip(handler, payload, reply_subject, nats))]
pub async fn handle<H, N>(handler: &H, payload: &[u8], reply_subject: Option<String>, nats: &N)
where
    H: A2aHandler,
    N: trogon_nats::PublishClient,
{
    let Some(reply) = reply_subject else {
        warn!("message/send received without reply subject; dropping");
        return;
    };

    let (id, result) = parse_and_call(handler, payload).await;
    let bytes = match result {
        Ok(resp) => JsonRpcResponse::new(id, resp).to_bytes(),
        Err(e) => JsonRpcErrorResponse::new(id, e.code, e.message).to_bytes(),
    };
    match bytes {
        Ok(b) => send_reply(nats, &reply, b).await,
        Err(e) => warn!(error = %e, "failed to serialize message/send response"),
    }
}

async fn parse_and_call<H: A2aHandler>(
    handler: &H,
    payload: &[u8],
) -> (Option<JsonRpcId>, Result<a2a::types::SendMessageResponse, A2aError>) {
    let req = match parse_request::<a2a::types::SendMessageRequest>(payload) {
        Ok(r) => r,
        Err(_) => return (None, Err(A2aError::internal("parse error"))),
    };
    let id = req.id;
    let params = match req.params {
        Some(p) => p,
        None => return (id, Err(A2aError::internal("missing params"))),
    };
    let result = handler.message_send(params).await;
    (id, result)
}

async fn send_reply<N: trogon_nats::PublishClient>(nats: &N, reply: &str, bytes: bytes::Bytes) {
    let subject = async_nats::Subject::from(reply);
    let headers = async_nats::HeaderMap::new();
    if let Err(e) = nats.publish_with_headers(subject, headers, bytes).await {
        warn!(error = %e, reply = %reply, "failed to publish message/send reply");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::handler::A2aError;
    use trogon_nats::AdvancedMockNatsClient;

    struct OkHandler;
    struct ErrHandler;

    #[async_trait::async_trait]
    impl A2aHandler for OkHandler {
        async fn message_send(
            &self,
            _req: a2a::types::SendMessageRequest,
        ) -> Result<a2a::types::SendMessageResponse, A2aError> {
            Ok(a2a::types::SendMessageResponse { payload: None })
        }
        async fn message_stream(
            &self,
            _req: a2a::types::SendMessageRequest,
        ) -> Result<(a2a::types::Task, crate::server::handler::TaskEventStream), A2aError> {
            Err(A2aError::unsupported_operation("stub"))
        }
        async fn tasks_get(&self, _req: a2a::types::GetTaskRequest) -> Result<a2a::types::Task, A2aError> {
            Err(A2aError::unsupported_operation("stub"))
        }
        async fn tasks_list(
            &self,
            _req: a2a::types::ListTasksRequest,
        ) -> Result<a2a::types::ListTasksResponse, A2aError> {
            Err(A2aError::unsupported_operation("stub"))
        }
        async fn tasks_cancel(&self, _req: a2a::types::CancelTaskRequest) -> Result<a2a::types::Task, A2aError> {
            Err(A2aError::unsupported_operation("stub"))
        }
        async fn tasks_resubscribe(
            &self,
            _req: a2a::types::SubscribeToTaskRequest,
        ) -> Result<a2a::types::Task, A2aError> {
            Err(A2aError::unsupported_operation("stub"))
        }
        async fn push_notification_set(
            &self,
            _req: a2a::types::TaskPushNotificationConfig,
        ) -> Result<a2a::types::TaskPushNotificationConfig, A2aError> {
            Err(A2aError::unsupported_operation("stub"))
        }
        async fn push_notification_get(
            &self,
            _req: a2a::types::GetTaskPushNotificationConfigRequest,
        ) -> Result<a2a::types::TaskPushNotificationConfig, A2aError> {
            Err(A2aError::unsupported_operation("stub"))
        }
        async fn push_notification_list(
            &self,
            _req: a2a::types::ListTaskPushNotificationConfigsRequest,
        ) -> Result<a2a::types::ListTaskPushNotificationConfigsResponse, A2aError> {
            Err(A2aError::unsupported_operation("stub"))
        }
        async fn push_notification_delete(
            &self,
            _req: a2a::types::DeleteTaskPushNotificationConfigRequest,
        ) -> Result<(), A2aError> {
            Err(A2aError::unsupported_operation("stub"))
        }
        async fn agent_card(
            &self,
            _req: a2a::types::GetExtendedAgentCardRequest,
        ) -> Result<a2a::agent_card::AgentCard, A2aError> {
            Err(A2aError::unsupported_operation("stub"))
        }
    }

    #[async_trait::async_trait]
    impl A2aHandler for ErrHandler {
        async fn message_send(
            &self,
            _req: a2a::types::SendMessageRequest,
        ) -> Result<a2a::types::SendMessageResponse, A2aError> {
            Err(A2aError::task_not_found("no task"))
        }
        async fn message_stream(
            &self,
            _req: a2a::types::SendMessageRequest,
        ) -> Result<(a2a::types::Task, crate::server::handler::TaskEventStream), A2aError> {
            Err(A2aError::unsupported_operation("stub"))
        }
        async fn tasks_get(&self, _req: a2a::types::GetTaskRequest) -> Result<a2a::types::Task, A2aError> {
            Err(A2aError::unsupported_operation("stub"))
        }
        async fn tasks_list(
            &self,
            _req: a2a::types::ListTasksRequest,
        ) -> Result<a2a::types::ListTasksResponse, A2aError> {
            Err(A2aError::unsupported_operation("stub"))
        }
        async fn tasks_cancel(&self, _req: a2a::types::CancelTaskRequest) -> Result<a2a::types::Task, A2aError> {
            Err(A2aError::unsupported_operation("stub"))
        }
        async fn tasks_resubscribe(
            &self,
            _req: a2a::types::SubscribeToTaskRequest,
        ) -> Result<a2a::types::Task, A2aError> {
            Err(A2aError::unsupported_operation("stub"))
        }
        async fn push_notification_set(
            &self,
            _req: a2a::types::TaskPushNotificationConfig,
        ) -> Result<a2a::types::TaskPushNotificationConfig, A2aError> {
            Err(A2aError::unsupported_operation("stub"))
        }
        async fn push_notification_get(
            &self,
            _req: a2a::types::GetTaskPushNotificationConfigRequest,
        ) -> Result<a2a::types::TaskPushNotificationConfig, A2aError> {
            Err(A2aError::unsupported_operation("stub"))
        }
        async fn push_notification_list(
            &self,
            _req: a2a::types::ListTaskPushNotificationConfigsRequest,
        ) -> Result<a2a::types::ListTaskPushNotificationConfigsResponse, A2aError> {
            Err(A2aError::unsupported_operation("stub"))
        }
        async fn push_notification_delete(
            &self,
            _req: a2a::types::DeleteTaskPushNotificationConfigRequest,
        ) -> Result<(), A2aError> {
            Err(A2aError::unsupported_operation("stub"))
        }
        async fn agent_card(
            &self,
            _req: a2a::types::GetExtendedAgentCardRequest,
        ) -> Result<a2a::agent_card::AgentCard, A2aError> {
            Err(A2aError::unsupported_operation("stub"))
        }
    }

    fn make_payload(id: i64) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "message/send",
            "params": {}
        }))
        .unwrap()
    }

    #[tokio::test]
    async fn success_publishes_result_to_reply() {
        let nats = AdvancedMockNatsClient::new();
        let payload = make_payload(1);
        handle(&OkHandler, &payload, Some("_INBOX.reply".into()), &nats).await;
        let published = nats.published_messages();
        assert_eq!(published, vec!["_INBOX.reply"]);
        let body: serde_json::Value = serde_json::from_slice(&nats.published_payloads()[0]).unwrap();
        assert_eq!(body["jsonrpc"], "2.0");
        assert!(body.get("result").is_some());
    }

    #[tokio::test]
    async fn error_handler_publishes_error_response() {
        let nats = AdvancedMockNatsClient::new();
        let payload = make_payload(2);
        handle(&ErrHandler, &payload, Some("_INBOX.reply".into()), &nats).await;
        let body: serde_json::Value = serde_json::from_slice(&nats.published_payloads()[0]).unwrap();
        assert_eq!(body["error"]["code"], crate::error::TASK_NOT_FOUND);
    }

    #[tokio::test]
    async fn missing_reply_subject_does_not_publish() {
        let nats = AdvancedMockNatsClient::new();
        let payload = make_payload(3);
        handle(&OkHandler, &payload, None, &nats).await;
        assert!(nats.published_messages().is_empty());
    }

    #[tokio::test]
    async fn malformed_payload_publishes_error() {
        let nats = AdvancedMockNatsClient::new();
        handle(&OkHandler, b"not json", Some("_INBOX.x".into()), &nats).await;
        let body: serde_json::Value = serde_json::from_slice(&nats.published_payloads()[0]).unwrap();
        assert!(body.get("error").is_some());
    }
}
