use a2a_nats::agent::{A2aError, A2aHandler, TaskEventStream};
use tracing::error;

struct NoopHandler;

#[async_trait::async_trait]
impl A2aHandler for NoopHandler {
    async fn message_send(
        &self,
        _req: a2a_types::SendMessageRequest,
    ) -> Result<a2a_types::SendMessageResponse, A2aError> {
        Err(A2aError::unsupported_operation("no handler registered"))
    }

    async fn message_stream(
        &self,
        _req: a2a_types::SendMessageRequest,
    ) -> Result<(a2a_types::Task, TaskEventStream), A2aError> {
        Err(A2aError::unsupported_operation("no handler registered"))
    }

    async fn tasks_get(&self, _req: a2a_types::GetTaskRequest) -> Result<a2a_types::Task, A2aError> {
        Err(A2aError::unsupported_operation("no handler registered"))
    }

    async fn tasks_list(&self, _req: a2a_types::ListTasksRequest) -> Result<a2a_types::ListTasksResponse, A2aError> {
        Err(A2aError::unsupported_operation("no handler registered"))
    }

    async fn tasks_cancel(&self, _req: a2a_types::CancelTaskRequest) -> Result<a2a_types::Task, A2aError> {
        Err(A2aError::unsupported_operation("no handler registered"))
    }

    async fn tasks_resubscribe(&self, _req: a2a_types::SubscribeToTaskRequest) -> Result<a2a_types::Task, A2aError> {
        Err(A2aError::unsupported_operation("no handler registered"))
    }

    async fn push_notification_set(
        &self,
        _req: a2a_types::TaskPushNotificationConfig,
    ) -> Result<a2a_types::TaskPushNotificationConfig, A2aError> {
        Err(A2aError::push_notification_not_supported("no handler registered"))
    }

    async fn push_notification_get(
        &self,
        _req: a2a_types::GetTaskPushNotificationConfigRequest,
    ) -> Result<a2a_types::TaskPushNotificationConfig, A2aError> {
        Err(A2aError::push_notification_not_supported("no handler registered"))
    }

    async fn push_notification_list(
        &self,
        _req: a2a_types::ListTaskPushNotificationConfigsRequest,
    ) -> Result<a2a_types::ListTaskPushNotificationConfigsResponse, A2aError> {
        Err(A2aError::push_notification_not_supported("no handler registered"))
    }

    async fn push_notification_delete(
        &self,
        _req: a2a_types::DeleteTaskPushNotificationConfigRequest,
    ) -> Result<(), A2aError> {
        Err(A2aError::push_notification_not_supported("no handler registered"))
    }

    async fn agent_card(&self, _req: a2a_types::GetExtendedAgentCardRequest) -> Result<a2a_types::AgentCard, A2aError> {
        Err(A2aError::unsupported_operation("no handler registered"))
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    if let Err(e) = a2a_nats_agent::run(NoopHandler).await {
        error!(error = %e, "A2A agent exited with error");
        std::process::exit(1);
    }
}
