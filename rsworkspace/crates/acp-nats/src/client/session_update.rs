use crate::session_id::AcpSessionId;
use agent_client_protocol::{Client, SessionNotification};
use tracing::{info, instrument, warn};

#[instrument(name = "acp.client.session.update", skip(payload, client), fields(session_id = %session_id))]
pub async fn handle<C: Client>(payload: &[u8], client: &C, session_id: &AcpSessionId) {
    info!(session_id = %session_id, "Forwarding session update to client");
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
        ContentBlock, ContentChunk, RequestPermissionRequest, RequestPermissionResponse,
        SessionUpdate,
    };
    use async_trait::async_trait;
    use std::cell::RefCell;

    fn session_id(s: &str) -> AcpSessionId {
        AcpSessionId::new(s).unwrap()
    }

    #[derive(Debug)]
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
        ) -> Result<(), agent_client_protocol::Error> {
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
        ) -> Result<RequestPermissionResponse, agent_client_protocol::Error> {
            Err(agent_client_protocol::Error::new(
                -32603,
                "not implemented in test mock",
            ))
        }
    }

    #[tokio::test]
    async fn session_update_forwards_notification_to_client() {
        let client = MockClient::new();
        let notification = SessionNotification::new(
            "session-001",
            SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::from("hello"))),
        );
        let payload = serde_json::to_vec(&notification).unwrap();

        handle(&payload, &client, &session_id("session-001")).await;

        assert_eq!(client.notification_count(), 1);
    }

    #[tokio::test]
    async fn session_update_invalid_payload_does_not_panic() {
        let client = MockClient::new();
        handle(b"not json", &client, &session_id("session-001")).await;
        assert_eq!(client.notification_count(), 0);
    }

    #[tokio::test]
    async fn session_update_client_error_does_not_panic() {
        let client = MockClient::failing();
        let notification = SessionNotification::new(
            "session-001",
            SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::from("hello"))),
        );
        let payload = serde_json::to_vec(&notification).unwrap();

        handle(&payload, &client, &session_id("session-001")).await;
    }

    #[tokio::test]
    async fn mock_client_request_permission_returns_err() {
        let client = MockClient::new();
        let req: RequestPermissionRequest = serde_json::from_value(serde_json::json!({
            "sessionId": "sess-1",
            "toolCall": { "toolCallId": "call-1" },
            "options": []
        }))
        .unwrap();
        let result = client.request_permission(req).await;
        assert!(result.is_err());
    }
}
