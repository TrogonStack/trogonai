//! Shared test scaffolding for `server/<op>.rs` modules.
//!
//! `StubHandler` grows a result slot per operation as each per-op PR lands.

use async_nats::header::HeaderMap;
use jsonrpc_nats::{Direction, Message, RequestId, ResponseId, decode, encode, to_json_value};

use crate::server::handler::{A2aError, A2aExecutor, TaskEventStream};

#[derive(Default)]
pub struct StubHandler {
    pub agent_card_result: Option<Result<a2a::agent_card::AgentCard, A2aError>>,
    pub message_send_result: Option<Result<a2a::types::SendMessageResponse, A2aError>>,
    pub tasks_get_result: Option<Result<a2a::types::Task, A2aError>>,
    pub tasks_list_result: Option<Result<a2a::types::ListTasksResponse, A2aError>>,
    pub tasks_cancel_result: Option<Result<a2a::types::Task, A2aError>>,
    pub tasks_resubscribe_result: Option<Result<a2a::types::Task, A2aError>>,
    pub push_set_result: Option<Result<a2a::types::TaskPushNotificationConfig, A2aError>>,
    pub push_get_result: Option<Result<a2a::types::TaskPushNotificationConfig, A2aError>>,
    pub push_list_result: Option<Result<a2a::types::ListTaskPushNotificationConfigsResponse, A2aError>>,
    pub push_delete_result: Option<Result<(), A2aError>>,
    pub message_stream_result: Option<Result<(a2a::types::Task, TaskEventStream), A2aError>>,
}

fn take_or_unimplemented<T>(slot: &mut Option<Result<T, A2aError>>) -> Result<T, A2aError> {
    slot.take()
        .unwrap_or_else(|| Err(A2aError::unsupported_operation("stub not configured")))
}

#[async_trait::async_trait]
impl A2aExecutor for std::sync::Mutex<StubHandler> {
    async fn agent_card(
        &self,
        _req: a2a::types::GetExtendedAgentCardRequest,
    ) -> Result<a2a::agent_card::AgentCard, A2aError> {
        take_or_unimplemented(&mut self.lock().unwrap().agent_card_result)
    }

    async fn message_send(
        &self,
        _req: a2a::types::SendMessageRequest,
    ) -> Result<a2a::types::SendMessageResponse, A2aError> {
        take_or_unimplemented(&mut self.lock().unwrap().message_send_result)
    }

    async fn tasks_get(&self, _req: a2a::types::GetTaskRequest) -> Result<a2a::types::Task, A2aError> {
        take_or_unimplemented(&mut self.lock().unwrap().tasks_get_result)
    }

    async fn tasks_list(&self, _req: a2a::types::ListTasksRequest) -> Result<a2a::types::ListTasksResponse, A2aError> {
        take_or_unimplemented(&mut self.lock().unwrap().tasks_list_result)
    }

    async fn tasks_cancel(&self, _req: a2a::types::CancelTaskRequest) -> Result<a2a::types::Task, A2aError> {
        take_or_unimplemented(&mut self.lock().unwrap().tasks_cancel_result)
    }

    async fn tasks_resubscribe(&self, _req: a2a::types::SubscribeToTaskRequest) -> Result<a2a::types::Task, A2aError> {
        take_or_unimplemented(&mut self.lock().unwrap().tasks_resubscribe_result)
    }

    async fn push_notification_set(
        &self,
        _req: a2a::types::TaskPushNotificationConfig,
    ) -> Result<a2a::types::TaskPushNotificationConfig, A2aError> {
        take_or_unimplemented(&mut self.lock().unwrap().push_set_result)
    }

    async fn push_notification_get(
        &self,
        _req: a2a::types::GetTaskPushNotificationConfigRequest,
    ) -> Result<a2a::types::TaskPushNotificationConfig, A2aError> {
        take_or_unimplemented(&mut self.lock().unwrap().push_get_result)
    }

    async fn push_notification_list(
        &self,
        _req: a2a::types::ListTaskPushNotificationConfigsRequest,
    ) -> Result<a2a::types::ListTaskPushNotificationConfigsResponse, A2aError> {
        take_or_unimplemented(&mut self.lock().unwrap().push_list_result)
    }

    async fn push_notification_delete(
        &self,
        _req: a2a::types::DeleteTaskPushNotificationConfigRequest,
    ) -> Result<(), A2aError> {
        take_or_unimplemented(&mut self.lock().unwrap().push_delete_result)
    }

    async fn message_stream(
        &self,
        _req: a2a::types::SendMessageRequest,
    ) -> Result<(a2a::types::Task, TaskEventStream), A2aError> {
        take_or_unimplemented(&mut self.lock().unwrap().message_stream_result)
    }
}

pub fn stub() -> std::sync::Mutex<StubHandler> {
    std::sync::Mutex::new(StubHandler::default())
}

pub fn wire_request(method: &str, id: RequestId, params: serde_json::Value) -> (HeaderMap, Vec<u8>) {
    let encoded = encode(&Message::Request {
        id,
        method: method.to_string(),
        params,
    })
    .expect("wire request must encode");
    (encoded.headers, encoded.body.to_vec())
}

pub fn wire_notification(method: &str, params: serde_json::Value) -> (HeaderMap, Vec<u8>) {
    let encoded = encode(&Message::Notification {
        method: method.to_string(),
        params,
    })
    .expect("wire notification must encode");
    (encoded.headers, encoded.body.to_vec())
}

pub fn rpc_payload(method: &str, id: i64) -> (HeaderMap, Vec<u8>) {
    wire_request(method, RequestId::Number(id), serde_json::json!({}))
}

pub fn parse_response(headers: &HeaderMap, body: &[u8]) -> serde_json::Value {
    let message = decode(Direction::Response, None, headers, body).expect("response must decode");
    to_json_value(&message)
}

pub fn parse_published_response(nats: &trogon_nats::AdvancedMockNatsClient, index: usize) -> serde_json::Value {
    parse_response(&nats.published_headers()[index], &nats.published_payloads()[index])
}

pub fn set_wire_success_response(
    mock: &trogon_nats::AdvancedMockNatsClient,
    subject: &str,
    id: ResponseId,
    result: serde_json::Value,
) {
    let encoded = encode(&Message::Success { id, result }).expect("wire success must encode");
    mock.set_response_wire(subject, encoded.headers, encoded.body);
}

pub fn set_wire_error_response(
    mock: &trogon_nats::AdvancedMockNatsClient,
    subject: &str,
    id: ResponseId,
    code: i32,
    message: &str,
) {
    let encoded = encode(&Message::Error {
        id,
        code,
        message: message.to_string(),
        data: None,
    })
    .expect("wire error must encode");
    mock.set_response_wire(subject, encoded.headers, encoded.body);
}
