use super::*;
use agent_client_protocol::schema::v1::{
    RequestPermissionRequest, RequestPermissionResponse, SessionNotification, ToolCallUpdate, ToolCallUpdateFields,
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
async fn mock_client_request_permission_returns_error() {
    let client = MockClient::new(None);
    let tool_call = ToolCallUpdate::new("id", ToolCallUpdateFields::new());
    let result = client
        .request_permission(RequestPermissionRequest::new("s1", tool_call, vec![]))
        .await;
    assert!(result.is_err());
}
