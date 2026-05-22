use std::pin::Pin;

use futures::Stream;

use crate::error::{
    AGENT_UNAVAILABLE, CONTENT_TYPE_NOT_SUPPORTED, INVALID_AGENT_RESPONSE, PUSH_NOTIFICATION_NOT_SUPPORTED,
    TASK_NOT_CANCELABLE, TASK_NOT_FOUND, UNSUPPORTED_OPERATION,
};

pub type TaskEventStream =
    Pin<Box<dyn Stream<Item = Result<a2a_types::StreamResponse, A2aError>> + Send + 'static>>;

/// Error returned by an [`A2aHandler`] implementation and mapped to a JSON-RPC error response.
#[derive(Debug)]
pub struct A2aError {
    pub code: i32,
    pub message: String,
}

impl A2aError {
    pub fn new(code: i32, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub fn task_not_found(message: impl Into<String>) -> Self {
        Self::new(TASK_NOT_FOUND, message)
    }

    pub fn task_not_cancelable(message: impl Into<String>) -> Self {
        Self::new(TASK_NOT_CANCELABLE, message)
    }

    pub fn push_notification_not_supported(message: impl Into<String>) -> Self {
        Self::new(PUSH_NOTIFICATION_NOT_SUPPORTED, message)
    }

    pub fn unsupported_operation(message: impl Into<String>) -> Self {
        Self::new(UNSUPPORTED_OPERATION, message)
    }

    pub fn content_type_not_supported(message: impl Into<String>) -> Self {
        Self::new(CONTENT_TYPE_NOT_SUPPORTED, message)
    }

    pub fn invalid_agent_response(message: impl Into<String>) -> Self {
        Self::new(INVALID_AGENT_RESPONSE, message)
    }

    pub fn agent_unavailable(message: impl Into<String>) -> Self {
        Self::new(AGENT_UNAVAILABLE, message)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(-32603, message)
    }
}

impl std::fmt::Display for A2aError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {}", self.code, self.message)
    }
}

impl std::error::Error for A2aError {}

/// Handler trait implemented by agent authors.
///
/// Every method maps 1-to-1 to an A2A JSON-RPC operation. For `message/stream`, the handler
/// returns a stream of [`a2a_types::StreamResponse`] items; the bridge consumes that stream,
/// publishes each item to `TaskEventsSubject` via JetStream, and sends the initial bootstrap
/// reply once the task ID is known (first item that contains a task or status must carry it).
///
/// `#[async_trait]` is applied so handler objects can be stored as `Box<dyn A2aHandler>` or
/// referenced generically without hand-rolling the trait bounds.
#[async_trait::async_trait]
pub trait A2aHandler: Send + Sync + 'static {
    async fn message_send(
        &self,
        request: a2a_types::SendMessageRequest,
    ) -> Result<a2a_types::SendMessageResponse, A2aError>;

    /// Bootstrap a streaming task. Returns the initial task envelope immediately and then
    /// yields [`a2a_types::StreamResponse`] events until the task reaches a terminal state.
    async fn message_stream(
        &self,
        request: a2a_types::SendMessageRequest,
    ) -> Result<(a2a_types::Task, TaskEventStream), A2aError>;

    async fn tasks_get(&self, request: a2a_types::GetTaskRequest) -> Result<a2a_types::Task, A2aError>;

    async fn tasks_list(&self, request: a2a_types::ListTasksRequest) -> Result<a2a_types::ListTasksResponse, A2aError>;

    async fn tasks_cancel(&self, request: a2a_types::CancelTaskRequest) -> Result<a2a_types::Task, A2aError>;

    async fn tasks_resubscribe(&self, request: a2a_types::SubscribeToTaskRequest) -> Result<a2a_types::Task, A2aError>;

    async fn push_notification_set(
        &self,
        request: a2a_types::TaskPushNotificationConfig,
    ) -> Result<a2a_types::TaskPushNotificationConfig, A2aError>;

    async fn push_notification_get(
        &self,
        request: a2a_types::GetTaskPushNotificationConfigRequest,
    ) -> Result<a2a_types::TaskPushNotificationConfig, A2aError>;

    async fn push_notification_list(
        &self,
        request: a2a_types::ListTaskPushNotificationConfigsRequest,
    ) -> Result<a2a_types::ListTaskPushNotificationConfigsResponse, A2aError>;

    async fn push_notification_delete(
        &self,
        request: a2a_types::DeleteTaskPushNotificationConfigRequest,
    ) -> Result<(), A2aError>;

    async fn agent_card(
        &self,
        request: a2a_types::GetExtendedAgentCardRequest,
    ) -> Result<a2a_types::AgentCard, A2aError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a2a_error_new() {
        let e = A2aError::new(-32001, "task not found");
        assert_eq!(e.code, -32001);
        assert_eq!(e.message, "task not found");
    }

    #[test]
    fn a2a_error_display() {
        let e = A2aError::new(-32001, "msg");
        assert_eq!(e.to_string(), "[-32001] msg");
    }

    #[test]
    fn a2a_error_constructors() {
        assert_eq!(A2aError::task_not_found("x").code, TASK_NOT_FOUND);
        assert_eq!(A2aError::task_not_cancelable("x").code, TASK_NOT_CANCELABLE);
        assert_eq!(
            A2aError::push_notification_not_supported("x").code,
            PUSH_NOTIFICATION_NOT_SUPPORTED
        );
        assert_eq!(A2aError::unsupported_operation("x").code, UNSUPPORTED_OPERATION);
        assert_eq!(A2aError::content_type_not_supported("x").code, CONTENT_TYPE_NOT_SUPPORTED);
        assert_eq!(A2aError::invalid_agent_response("x").code, INVALID_AGENT_RESPONSE);
        assert_eq!(A2aError::agent_unavailable("x").code, AGENT_UNAVAILABLE);
        assert_eq!(A2aError::internal("x").code, -32603);
    }

    #[test]
    fn a2a_error_implements_std_error() {
        let e: Box<dyn std::error::Error> = Box::new(A2aError::new(-32001, "oops"));
        assert!(e.to_string().contains("oops"));
    }
}
