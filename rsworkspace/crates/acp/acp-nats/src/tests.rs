use super::*;
use agent_client_protocol::schema::v1::{
    ContentBlock, ContentChunk, RequestPermissionRequest, RequestPermissionResponse, SessionNotification,
    SessionUpdate, TextContent, ToolCallUpdate, ToolCallUpdateFields,
};
use std::sync::{Arc, Mutex};

struct MockClient {
    received: Arc<Mutex<Vec<SessionNotification>>>,
    fail_after: Option<usize>,
}

impl MockClient {
    fn new(fail_after: Option<usize>) -> Self {
        Self {
            received: Arc::new(Mutex::new(Vec::new())),
            fail_after,
        }
    }
}

#[async_trait::async_trait]
impl crate::ClientHandler for MockClient {
    async fn request_permission(
        &self,
        _args: RequestPermissionRequest,
    ) -> agent_client_protocol::Result<RequestPermissionResponse> {
        Err(agent_client_protocol::Error::internal_error())
    }

    async fn session_notification(&self, args: SessionNotification) -> agent_client_protocol::Result<()> {
        let mut received = self.received.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let count = received.len();
        if let Some(limit) = self.fail_after
            && count >= limit
        {
            return Err(agent_client_protocol::Error::internal_error());
        }
        received.push(args);
        Ok(())
    }
}

#[tokio::test]
async fn spawn_notification_forwarder_delivers_notifications() {
    let client = MockClient::new(None);
    let received = client.received.clone();
    let (tx, rx) = tokio::sync::mpsc::channel(16);

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            spawn_notification_forwarder(client, rx);

            let notif = SessionNotification::new(
                "s1",
                SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(TextContent::new("hello")))),
            );
            tx.send(notif).await.unwrap();
            drop(tx);

            tokio::task::yield_now().await;
            tokio::task::yield_now().await;
        })
        .await;

    assert_eq!(received.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn mock_client_request_permission_returns_error() {
    let client = MockClient::new(None);
    let tool_call = ToolCallUpdate::new("id", ToolCallUpdateFields::new());
    let result = client
        .request_permission(RequestPermissionRequest::new("s1", tool_call, vec![]))
        .await;
    assert!(result.is_err());
}

#[tokio::test]
async fn spawn_notification_forwarder_stops_on_client_error() {
    let client = MockClient::new(Some(0));
    let received = client.received.clone();
    let (tx, rx) = tokio::sync::mpsc::channel(16);

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            spawn_notification_forwarder(client, rx);

            let notif = SessionNotification::new(
                "s1",
                SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(TextContent::new("hello")))),
            );
            let _ = tx.send(notif).await;
            drop(tx);

            tokio::task::yield_now().await;
            tokio::task::yield_now().await;
        })
        .await;

    assert_eq!(received.lock().unwrap().len(), 0);
}
