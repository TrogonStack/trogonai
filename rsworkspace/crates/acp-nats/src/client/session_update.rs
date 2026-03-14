use agent_client_protocol::{Client, SessionNotification};
use tracing::{instrument, warn};

#[instrument(name = "acp.client.session.update", skip(payload, client))]
pub async fn handle<C: Client>(payload: &[u8], client: &C, has_reply: bool) {
    if has_reply {
        warn!("Unexpected reply subject on notification request");
    }
    match serde_json::from_slice::<SessionNotification>(payload) {
        Ok(notification) => {
            if let Err(e) = client.session_notification(notification).await {
                warn!(error = %e, "Failed to send session notification");
            }
        }
        Err(e) => {
            warn!(error = %e, "Failed to parse session notification");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::{
        ContentBlock, ContentChunk, RequestPermissionOutcome, RequestPermissionRequest,
        RequestPermissionResponse, SessionUpdate,
    };
    use async_trait::async_trait;
    use std::cell::RefCell;

    struct MockClient {
        notifications_received: RefCell<Vec<String>>,
        should_fail: bool,
    }

    impl MockClient {
        fn new() -> Self {
            Self {
                notifications_received: RefCell::new(Vec::new()),
                should_fail: false,
            }
        }

        fn failing() -> Self {
            Self {
                notifications_received: RefCell::new(Vec::new()),
                should_fail: true,
            }
        }

        fn notification_count(&self) -> usize {
            self.notifications_received.borrow().len()
        }
    }

    #[async_trait(?Send)]
    impl Client for MockClient {
        async fn session_notification(
            &self,
            notification: SessionNotification,
        ) -> agent_client_protocol::Result<()> {
            if self.should_fail {
                return Err(agent_client_protocol::Error::new(-1, "mock failure"));
            }
            self.notifications_received
                .borrow_mut()
                .push(format!("{:?}", notification));
            Ok(())
        }

        async fn request_permission(
            &self,
            _: RequestPermissionRequest,
        ) -> agent_client_protocol::Result<RequestPermissionResponse> {
            Ok(RequestPermissionResponse::new(
                RequestPermissionOutcome::Cancelled,
            ))
        }
    }

    #[tokio::test]
    async fn forwards_notification_to_client() {
        let client = MockClient::new();
        let notification = SessionNotification::new(
            "session-001",
            SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::from("hello"))),
        );
        let payload = serde_json::to_vec(&notification).unwrap();

        handle(&payload, &client, false).await;

        assert_eq!(client.notification_count(), 1);
    }

    #[tokio::test]
    async fn invalid_payload_does_not_panic() {
        let client = MockClient::new();
        handle(b"not json", &client, false).await;
        assert_eq!(client.notification_count(), 0);
    }

    #[tokio::test]
    async fn client_error_does_not_panic() {
        let client = MockClient::failing();
        let notification = SessionNotification::new(
            "session-001",
            SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::from("hello"))),
        );
        let payload = serde_json::to_vec(&notification).unwrap();

        handle(&payload, &client, false).await;
    }

    #[tokio::test]
    async fn has_reply_logs_warning_but_still_forwards() {
        let client = MockClient::new();
        let notification = SessionNotification::new(
            "session-001",
            SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::from("hello"))),
        );
        let payload = serde_json::to_vec(&notification).unwrap();

        handle(&payload, &client, true).await;

        assert_eq!(client.notification_count(), 1);
    }

    #[tokio::test]
    async fn mock_client_trait_coverage() {
        use agent_client_protocol::{ToolCallUpdate, ToolCallUpdateFields};

        let client = MockClient::new();
        let tool_call = ToolCallUpdate::new("call-1", ToolCallUpdateFields::new());
        let req = RequestPermissionRequest::new("sess-1", tool_call, vec![]);
        assert!(client.request_permission(req).await.is_ok());
    }
}
