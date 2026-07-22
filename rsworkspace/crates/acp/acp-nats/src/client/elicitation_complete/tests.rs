use super::*;
use agent_client_protocol::schema::v1::{
    CreateElicitationRequest, CreateElicitationResponse, ElicitationAcceptAction, RequestPermissionOutcome,
    RequestPermissionRequest, RequestPermissionResponse,
};
use async_nats::header::HeaderMap;
use async_trait::async_trait;
use std::sync::Mutex;

struct MockClient {
    completions_received: Mutex<Vec<String>>,
    should_fail: bool,
}

impl MockClient {
    fn new() -> Self {
        Self {
            completions_received: Mutex::new(Vec::new()),
            should_fail: false,
        }
    }

    fn failing() -> Self {
        Self {
            completions_received: Mutex::new(Vec::new()),
            should_fail: true,
        }
    }

    fn completion_count(&self) -> usize {
        self.completions_received.lock().unwrap().len()
    }
}

#[async_trait]
impl ClientHandler for MockClient {
    async fn session_notification(
        &self,
        _: agent_client_protocol::schema::v1::SessionNotification,
    ) -> agent_client_protocol::Result<()> {
        Ok(())
    }

    async fn request_permission(
        &self,
        _: RequestPermissionRequest,
    ) -> agent_client_protocol::Result<RequestPermissionResponse> {
        Ok(RequestPermissionResponse::new(RequestPermissionOutcome::Cancelled))
    }

    async fn elicitation_complete(
        &self,
        notification: CompleteElicitationNotification,
    ) -> agent_client_protocol::Result<()> {
        if self.should_fail {
            return Err(agent_client_protocol::Error::new(-1, "mock failure"));
        }
        self.completions_received
            .lock()
            .unwrap()
            .push(format!("{:?}", notification));
        Ok(())
    }
}

fn empty_headers() -> HeaderMap {
    HeaderMap::new()
}

fn test_metrics() -> crate::telemetry::metrics::Metrics {
    crate::telemetry::metrics::Metrics::new(&opentelemetry::global::meter("elicitation-complete-test"))
}

#[tokio::test]
async fn forwards_notification_to_client() {
    let client = MockClient::new();
    let notification = CompleteElicitationNotification::new("elicitation-1");
    let (headers, payload) =
        crate::client::test_support::encode_wire_notification("elicitation/complete", &notification);

    handle(&headers, &payload, &client, false, &test_metrics()).await;

    assert_eq!(client.completion_count(), 1);
}

#[tokio::test]
async fn invalid_payload_does_not_panic() {
    let client = MockClient::new();
    handle(&empty_headers(), b"not json", &client, false, &test_metrics()).await;
    assert_eq!(client.completion_count(), 0);
}

#[tokio::test]
async fn client_error_does_not_panic() {
    let client = MockClient::failing();
    let notification = CompleteElicitationNotification::new("elicitation-1");
    let (headers, payload) =
        crate::client::test_support::encode_wire_notification("elicitation/complete", &notification);

    handle(&headers, &payload, &client, false, &test_metrics()).await;
}

#[tokio::test]
async fn has_reply_logs_warning_but_still_forwards() {
    let client = MockClient::new();
    let notification = CompleteElicitationNotification::new("elicitation-1");
    let (headers, payload) =
        crate::client::test_support::encode_wire_notification("elicitation/complete", &notification);

    handle(&headers, &payload, &client, true, &test_metrics()).await;

    assert_eq!(client.completion_count(), 1);
}

#[tokio::test]
async fn mock_client_trait_coverage() {
    let client = MockClient::new();
    let action = ElicitationAcceptAction::new();
    let mode = agent_client_protocol::schema::v1::ElicitationFormMode::new(
        agent_client_protocol::schema::v1::ElicitationSessionScope::new("sess-1"),
        agent_client_protocol::schema::v1::ElicitationSchema::new(),
    );
    let req = CreateElicitationRequest::new(mode, "please respond");
    let resp = client.elicitation_create(req).await;
    assert!(resp.is_err());
    let _ = CreateElicitationResponse::new(action);
}
