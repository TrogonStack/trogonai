use crate::agent::handler::{A2aError, A2aHandler, TaskEventStream};

/// A minimal handler that always returns an error for every method.
/// Tests override only the method they care about by wrapping this.
#[derive(Default)]
pub struct StubHandler {
    pub message_send_result: Option<Result<a2a_types::SendMessageResponse, A2aError>>,
    pub tasks_get_result: Option<Result<a2a_types::Task, A2aError>>,
    pub tasks_list_result: Option<Result<a2a_types::ListTasksResponse, A2aError>>,
    pub tasks_cancel_result: Option<Result<a2a_types::Task, A2aError>>,
    pub tasks_resubscribe_result: Option<Result<a2a_types::Task, A2aError>>,
    pub agent_card_result: Option<Result<a2a_types::AgentCard, A2aError>>,
    pub push_set_result: Option<Result<a2a_types::TaskPushNotificationConfig, A2aError>>,
    pub push_get_result: Option<Result<a2a_types::TaskPushNotificationConfig, A2aError>>,
    pub push_list_result: Option<Result<a2a_types::ListTaskPushNotificationConfigsResponse, A2aError>>,
    pub push_delete_result: Option<Result<(), A2aError>>,
}

fn take_or_unimplemented<T>(slot: &mut Option<Result<T, A2aError>>) -> Result<T, A2aError> {
    slot.take()
        .unwrap_or_else(|| Err(A2aError::unsupported_operation("stub not configured")))
}

#[async_trait::async_trait]
impl A2aHandler for std::sync::Mutex<StubHandler> {
    async fn message_send(
        &self,
        _req: a2a_types::SendMessageRequest,
    ) -> Result<a2a_types::SendMessageResponse, A2aError> {
        take_or_unimplemented(&mut self.lock().unwrap().message_send_result)
    }

    async fn message_stream(
        &self,
        _req: a2a_types::SendMessageRequest,
    ) -> Result<(a2a_types::Task, TaskEventStream), A2aError> {
        Err(A2aError::unsupported_operation("stub not configured"))
    }

    async fn tasks_get(&self, _req: a2a_types::GetTaskRequest) -> Result<a2a_types::Task, A2aError> {
        take_or_unimplemented(&mut self.lock().unwrap().tasks_get_result)
    }

    async fn tasks_list(&self, _req: a2a_types::ListTasksRequest) -> Result<a2a_types::ListTasksResponse, A2aError> {
        take_or_unimplemented(&mut self.lock().unwrap().tasks_list_result)
    }

    async fn tasks_cancel(&self, _req: a2a_types::CancelTaskRequest) -> Result<a2a_types::Task, A2aError> {
        take_or_unimplemented(&mut self.lock().unwrap().tasks_cancel_result)
    }

    async fn tasks_resubscribe(&self, _req: a2a_types::SubscribeToTaskRequest) -> Result<a2a_types::Task, A2aError> {
        take_or_unimplemented(&mut self.lock().unwrap().tasks_resubscribe_result)
    }

    async fn push_notification_set(
        &self,
        _req: a2a_types::TaskPushNotificationConfig,
    ) -> Result<a2a_types::TaskPushNotificationConfig, A2aError> {
        take_or_unimplemented(&mut self.lock().unwrap().push_set_result)
    }

    async fn push_notification_get(
        &self,
        _req: a2a_types::GetTaskPushNotificationConfigRequest,
    ) -> Result<a2a_types::TaskPushNotificationConfig, A2aError> {
        take_or_unimplemented(&mut self.lock().unwrap().push_get_result)
    }

    async fn push_notification_list(
        &self,
        _req: a2a_types::ListTaskPushNotificationConfigsRequest,
    ) -> Result<a2a_types::ListTaskPushNotificationConfigsResponse, A2aError> {
        take_or_unimplemented(&mut self.lock().unwrap().push_list_result)
    }

    async fn push_notification_delete(
        &self,
        _req: a2a_types::DeleteTaskPushNotificationConfigRequest,
    ) -> Result<(), A2aError> {
        take_or_unimplemented(&mut self.lock().unwrap().push_delete_result)
    }

    async fn agent_card(&self, _req: a2a_types::GetExtendedAgentCardRequest) -> Result<a2a_types::AgentCard, A2aError> {
        take_or_unimplemented(&mut self.lock().unwrap().agent_card_result)
    }
}

pub fn stub() -> std::sync::Mutex<StubHandler> {
    std::sync::Mutex::new(StubHandler::default())
}

pub fn make_task(id: &str) -> a2a_types::Task {
    a2a_types::Task {
        id: id.to_string(),
        ..Default::default()
    }
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
