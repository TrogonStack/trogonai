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
mod tests {
    use super::*;

    fn task_id(id: &str) -> a2a::types::TaskId {
        id.to_string()
    }

    #[tokio::test]
    async fn message_send_returns_unsupported_operation() {
        let h = NoopHandler;
        let req = a2a::types::SendMessageRequest {
            message: a2a::types::Message {
                message_id: "m".into(),
                role: a2a::types::Role::User,
                parts: vec![],
                context_id: None,
                task_id: None,
                reference_task_ids: None,
                extensions: None,
                metadata: None,
            },
            configuration: None,
            metadata: None,
            tenant: None,
        };
        assert_eq!(
            h.message_send(req).await.unwrap_err().code,
            a2a_nats::error::UNSUPPORTED_OPERATION
        );
    }

    #[tokio::test]
    async fn message_stream_returns_unsupported_operation() {
        let h = NoopHandler;
        let req = a2a::types::SendMessageRequest {
            message: a2a::types::Message {
                message_id: "m".into(),
                role: a2a::types::Role::User,
                parts: vec![],
                context_id: None,
                task_id: None,
                reference_task_ids: None,
                extensions: None,
                metadata: None,
            },
            configuration: None,
            metadata: None,
            tenant: None,
        };
        // TaskEventStream isn't Debug, so we can't `unwrap_err()` on a (Task, TaskEventStream) tuple.
        match h.message_stream(req).await {
            Ok(_) => panic!("expected error"),
            Err(e) => assert_eq!(e.code, a2a_nats::error::UNSUPPORTED_OPERATION),
        }
    }

    #[tokio::test]
    async fn tasks_get_returns_unsupported_operation() {
        let h = NoopHandler;
        let req = a2a::types::GetTaskRequest {
            id: task_id("t"),
            tenant: None,
            history_length: None,
        };
        assert_eq!(
            h.tasks_get(req).await.unwrap_err().code,
            a2a_nats::error::UNSUPPORTED_OPERATION
        );
    }

    #[tokio::test]
    async fn tasks_list_returns_unsupported_operation() {
        let h = NoopHandler;
        let req = a2a::types::ListTasksRequest {
            context_id: None,
            status: None,
            page_size: None,
            page_token: None,
            history_length: None,
            status_timestamp_after: None,
            include_artifacts: None,
            tenant: None,
        };
        assert_eq!(
            h.tasks_list(req).await.unwrap_err().code,
            a2a_nats::error::UNSUPPORTED_OPERATION
        );
    }

    #[tokio::test]
    async fn tasks_cancel_returns_unsupported_operation() {
        let h = NoopHandler;
        let req = a2a::types::CancelTaskRequest {
            id: task_id("t"),
            metadata: None,
            tenant: None,
        };
        assert_eq!(
            h.tasks_cancel(req).await.unwrap_err().code,
            a2a_nats::error::UNSUPPORTED_OPERATION
        );
    }

    #[tokio::test]
    async fn tasks_resubscribe_returns_unsupported_operation() {
        let h = NoopHandler;
        let req = a2a::types::SubscribeToTaskRequest {
            id: task_id("t"),
            tenant: None,
        };
        assert_eq!(
            h.tasks_resubscribe(req).await.unwrap_err().code,
            a2a_nats::error::UNSUPPORTED_OPERATION
        );
    }

    fn push_config() -> a2a::types::TaskPushNotificationConfig {
        a2a::types::TaskPushNotificationConfig {
            url: "https://example.com".into(),
            id: Some("c".into()),
            task_id: task_id("t"),
            token: None,
            authentication: None,
            tenant: None,
        }
    }

    #[tokio::test]
    async fn push_set_returns_push_not_supported() {
        let h = NoopHandler;
        assert_eq!(
            h.push_notification_set(push_config()).await.unwrap_err().code,
            a2a_nats::error::PUSH_NOTIFICATION_NOT_SUPPORTED
        );
    }

    #[tokio::test]
    async fn push_get_returns_push_not_supported() {
        let h = NoopHandler;
        let req = a2a::types::GetTaskPushNotificationConfigRequest {
            task_id: task_id("t"),
            id: "c".into(),
            tenant: None,
        };
        assert_eq!(
            h.push_notification_get(req).await.unwrap_err().code,
            a2a_nats::error::PUSH_NOTIFICATION_NOT_SUPPORTED
        );
    }

    #[tokio::test]
    async fn push_list_returns_push_not_supported() {
        let h = NoopHandler;
        let req = a2a::types::ListTaskPushNotificationConfigsRequest {
            task_id: task_id("t"),
            page_size: None,
            page_token: None,
            tenant: None,
        };
        assert_eq!(
            h.push_notification_list(req).await.unwrap_err().code,
            a2a_nats::error::PUSH_NOTIFICATION_NOT_SUPPORTED
        );
    }

    #[tokio::test]
    async fn push_delete_returns_push_not_supported() {
        let h = NoopHandler;
        let req = a2a::types::DeleteTaskPushNotificationConfigRequest {
            task_id: task_id("t"),
            id: "c".into(),
            tenant: None,
        };
        assert_eq!(
            h.push_notification_delete(req).await.unwrap_err().code,
            a2a_nats::error::PUSH_NOTIFICATION_NOT_SUPPORTED
        );
    }

    #[tokio::test]
    async fn agent_card_returns_unsupported_operation() {
        let h = NoopHandler;
        let req = a2a::types::GetExtendedAgentCardRequest { tenant: None };
        assert_eq!(
            h.agent_card(req).await.unwrap_err().code,
            a2a_nats::error::UNSUPPORTED_OPERATION
        );
    }
}
