//! Shared test scaffolding for `server/<op>.rs` modules.
//!
//! `StubHandler` grows a result slot per operation as each per-op PR lands.

use crate::server::handler::{A2aError, A2aExecutor};

#[derive(Default)]
pub struct StubHandler {
    pub agent_card_result: Option<Result<a2a::agent_card::AgentCard, A2aError>>,
    pub message_send_result: Option<Result<a2a::types::SendMessageResponse, A2aError>>,
    pub tasks_get_result: Option<Result<a2a::types::Task, A2aError>>,
    pub tasks_list_result: Option<Result<a2a::types::ListTasksResponse, A2aError>>,
    pub tasks_cancel_result: Option<Result<a2a::types::Task, A2aError>>,
    pub tasks_resubscribe_result: Option<Result<a2a::types::Task, A2aError>>,
    pub push_set_result: Option<Result<a2a::types::TaskPushNotificationConfig, A2aError>>,
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
}

pub fn stub() -> std::sync::Mutex<StubHandler> {
    std::sync::Mutex::new(StubHandler::default())
}

pub fn rpc_payload(method: &str, id: i64) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": {}
    }))
    .unwrap()
}

pub fn parse_response(bytes: &[u8]) -> serde_json::Value {
    serde_json::from_slice(bytes).expect("response must be valid JSON")
}
