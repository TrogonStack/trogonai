//! Default no-op `A2aExecutor` implementation used by `a2a-nats-server` when
//! the operator runs the binary without registering their own handler.
//!
//! Every method returns `unsupported_operation` (or
//! `push_notification_not_supported` for push ops) so callers see a typed
//! JSON-RPC error rather than silence.

use a2a_nats::server::{A2aError, A2aExecutor, TaskEventStream};

pub struct NoopHandler;

#[async_trait::async_trait]
impl A2aExecutor for NoopHandler {
    async fn message_send(
        &self,
        _req: a2a::types::SendMessageRequest,
    ) -> Result<a2a::types::SendMessageResponse, A2aError> {
        Err(A2aError::unsupported_operation("no handler registered"))
    }

    async fn message_stream(
        &self,
        _req: a2a::types::SendMessageRequest,
    ) -> Result<(a2a::types::Task, TaskEventStream), A2aError> {
        Err(A2aError::unsupported_operation("no handler registered"))
    }

    async fn tasks_get(&self, _req: a2a::types::GetTaskRequest) -> Result<a2a::types::Task, A2aError> {
        Err(A2aError::unsupported_operation("no handler registered"))
    }

    async fn tasks_list(&self, _req: a2a::types::ListTasksRequest) -> Result<a2a::types::ListTasksResponse, A2aError> {
        Err(A2aError::unsupported_operation("no handler registered"))
    }

    async fn tasks_cancel(&self, _req: a2a::types::CancelTaskRequest) -> Result<a2a::types::Task, A2aError> {
        Err(A2aError::unsupported_operation("no handler registered"))
    }

    async fn tasks_resubscribe(&self, _req: a2a::types::SubscribeToTaskRequest) -> Result<a2a::types::Task, A2aError> {
        Err(A2aError::unsupported_operation("no handler registered"))
    }

    async fn push_notification_set(
        &self,
        _req: a2a::types::TaskPushNotificationConfig,
    ) -> Result<a2a::types::TaskPushNotificationConfig, A2aError> {
        Err(A2aError::push_notification_not_supported("no handler registered"))
    }

    async fn push_notification_get(
        &self,
        _req: a2a::types::GetTaskPushNotificationConfigRequest,
    ) -> Result<a2a::types::TaskPushNotificationConfig, A2aError> {
        Err(A2aError::push_notification_not_supported("no handler registered"))
    }

    async fn push_notification_list(
        &self,
        _req: a2a::types::ListTaskPushNotificationConfigsRequest,
    ) -> Result<a2a::types::ListTaskPushNotificationConfigsResponse, A2aError> {
        Err(A2aError::push_notification_not_supported("no handler registered"))
    }

    async fn push_notification_delete(
        &self,
        _req: a2a::types::DeleteTaskPushNotificationConfigRequest,
    ) -> Result<(), A2aError> {
        Err(A2aError::push_notification_not_supported("no handler registered"))
    }

    async fn agent_card(
        &self,
        _req: a2a::types::GetExtendedAgentCardRequest,
    ) -> Result<a2a::agent_card::AgentCard, A2aError> {
        Err(A2aError::unsupported_operation("no handler registered"))
    }
}

#[cfg(test)]
mod tests;
