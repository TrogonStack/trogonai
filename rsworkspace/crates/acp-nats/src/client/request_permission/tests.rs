use super::*;
use agent_client_protocol::schema::v1::{
    ContentBlock, ContentChunk, PermissionOption, PermissionOptionKind, RequestPermissionOutcome,
    RequestPermissionResponse, SessionNotification, SessionUpdate, ToolCallUpdate, ToolCallUpdateFields,
};
use async_nats::header::HeaderMap;
use jsonrpc_nats::RequestId;
use std::error::Error;
use trogon_nats::{AdvancedMockNatsClient, MockNatsClient};

struct MockClient {
    outcome: RequestPermissionOutcome,
}

impl MockClient {
    fn new(outcome: RequestPermissionOutcome) -> Self {
        Self { outcome }
    }
}

#[async_trait::async_trait]
impl ClientHandler for MockClient {
    async fn session_notification(&self, _: SessionNotification) -> agent_client_protocol::Result<()> {
        Ok(())
    }

    async fn request_permission(
        &self,
        _: RequestPermissionRequest,
    ) -> agent_client_protocol::Result<RequestPermissionResponse> {
        Ok(RequestPermissionResponse::new(self.outcome.clone()))
    }
}

struct FailingClient;

#[async_trait::async_trait]
impl ClientHandler for FailingClient {
    async fn session_notification(&self, _: SessionNotification) -> agent_client_protocol::Result<()> {
        Ok(())
    }

    async fn request_permission(
        &self,
        _: RequestPermissionRequest,
    ) -> agent_client_protocol::Result<RequestPermissionResponse> {
        Err(agent_client_protocol::Error::new(
            i32::from(ErrorCode::InvalidParams),
            "permission denied",
        ))
    }
}

fn make_wire_request(request: RequestPermissionRequest) -> (HeaderMap, Vec<u8>) {
    crate::client::test_support::encode_wire_request("session/request_permission", RequestId::Number(1), &request)
}

fn empty_headers() -> HeaderMap {
    HeaderMap::new()
}

#[tokio::test]
async fn mock_client_session_notification_returns_ok() {
    let client = MockClient::new(RequestPermissionOutcome::Cancelled);
    let notification = SessionNotification::new(
        "sess-1",
        SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::from("hi"))),
    );
    let result = client.session_notification(notification).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn failing_client_session_notification_returns_ok() {
    let client = FailingClient;
    let notification = SessionNotification::new(
        "sess-1",
        SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::from("hi"))),
    );
    let result = client.session_notification(notification).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn request_permission_forwards_request_and_returns_response() {
    let client = MockClient::new(RequestPermissionOutcome::Cancelled);
    let tool_call = ToolCallUpdate::new("call-1", ToolCallUpdateFields::new());
    let options = vec![PermissionOption::new(
        "allow-once",
        "Allow once",
        PermissionOptionKind::AllowOnce,
    )];
    let request = RequestPermissionRequest::new("session-001", tool_call, options);
    let (headers, payload) = make_wire_request(request);

    let result = forward_to_client(&headers, &payload, &client, "session-001").await;
    assert!(result.is_ok());
    let response = result.unwrap();
    assert_eq!(response.outcome, RequestPermissionOutcome::Cancelled);
}

#[tokio::test]
async fn request_permission_returns_error_when_payload_is_invalid_json() {
    let client = MockClient::new(RequestPermissionOutcome::Cancelled);
    let result = forward_to_client(&empty_headers(), b"not json", &client, "session-001").await;
    assert!(result.is_err());
}

#[tokio::test]
async fn request_permission_returns_client_error_when_client_fails() {
    let client = FailingClient;
    let tool_call = ToolCallUpdate::new("call-1", ToolCallUpdateFields::new());
    let request = RequestPermissionRequest::new("session-001", tool_call, vec![]);
    let (headers, payload) = make_wire_request(request);

    let result = forward_to_client(&headers, &payload, &client, "session-001").await;
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), RequestPermissionError::ClientError(_)));
}

#[tokio::test]
async fn request_permission_returns_invalid_request_when_params_missing() {
    let client = MockClient::new(RequestPermissionOutcome::Cancelled);
    let result = forward_to_client(&empty_headers(), b"{}", &client, "session-001").await;
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), RequestPermissionError::InvalidRequest(_)));
}

#[tokio::test]
async fn request_permission_returns_invalid_request_when_session_id_mismatch() {
    let client = MockClient::new(RequestPermissionOutcome::Cancelled);
    let tool_call = ToolCallUpdate::new("call-1", ToolCallUpdateFields::new());
    let request = RequestPermissionRequest::new("session-other", tool_call, vec![]);
    let (headers, payload) = make_wire_request(request);

    let result = forward_to_client(&headers, &payload, &client, "session-001").await;
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), RequestPermissionError::InvalidRequest(_)));
}

#[test]
fn error_code_and_message_invalid_request_returns_invalid_params() {
    let err = serde_json::from_slice::<RequestPermissionRequest>(b"not json").unwrap_err();
    let rp_err = RequestPermissionError::InvalidRequest(err);
    let (code, _) = error_code_and_message(&rp_err);
    assert_eq!(code, ErrorCode::InvalidParams);
}

#[test]
fn error_code_and_message_client_error_preserves_client_code() {
    let client_err = agent_client_protocol::Error::new(ErrorCode::InvalidParams.into(), "denied");
    let rp_err = RequestPermissionError::ClientError(client_err);
    let (code, message) = error_code_and_message(&rp_err);
    assert_eq!(code, ErrorCode::InvalidParams);
    assert_eq!(message, "denied");
}

#[test]
fn request_permission_error_display() {
    let err = serde_json::from_slice::<RequestPermissionRequest>(b"not json").unwrap_err();
    let expected = format!("invalid request: {err}");
    let rp_err = RequestPermissionError::InvalidRequest(err);
    assert_eq!(rp_err.to_string(), expected);

    let client_err = agent_client_protocol::Error::new(ErrorCode::InvalidParams.into(), "permission denied");
    let rp_err = RequestPermissionError::ClientError(client_err);
    assert_eq!(rp_err.to_string(), "client error: permission denied");
}

#[test]
fn request_permission_error_source() {
    let err = serde_json::from_slice::<RequestPermissionRequest>(b"not json").unwrap_err();
    let rp_err = RequestPermissionError::InvalidRequest(err);
    assert!(rp_err.source().is_some());

    let client_err = agent_client_protocol::Error::new(ErrorCode::InvalidParams.into(), "denied");
    let rp_err = RequestPermissionError::ClientError(client_err);
    assert!(rp_err.source().is_some());
}

#[tokio::test]
async fn handle_success_publishes_response_to_reply_subject() {
    let nats = MockNatsClient::new();
    let client = MockClient::new(RequestPermissionOutcome::Cancelled);
    let tool_call = ToolCallUpdate::new("call-1", ToolCallUpdateFields::new());
    let request = RequestPermissionRequest::new("session-001", tool_call, vec![]);
    let (headers, payload) = make_wire_request(request);

    handle(&headers, &payload, &client, Some("_INBOX.reply"), &nats, "session-001").await;

    assert_eq!(nats.published_messages(), vec!["_INBOX.reply"]);
}

#[tokio::test]
async fn handle_no_reply_does_not_publish() {
    let nats = MockNatsClient::new();
    let client = MockClient::new(RequestPermissionOutcome::Cancelled);
    let tool_call = ToolCallUpdate::new("call-1", ToolCallUpdateFields::new());
    let request = RequestPermissionRequest::new("session-001", tool_call, vec![]);
    let (headers, payload) = make_wire_request(request);

    handle(&headers, &payload, &client, None, &nats, "session-001").await;

    assert!(nats.published_messages().is_empty());
}

#[tokio::test]
async fn handle_session_id_mismatch_publishes_error_reply() {
    let nats = MockNatsClient::new();
    let client = MockClient::new(RequestPermissionOutcome::Cancelled);
    let tool_call = ToolCallUpdate::new("call-1", ToolCallUpdateFields::new());
    let request = RequestPermissionRequest::new("session-other", tool_call, vec![]);
    let (headers, payload) = make_wire_request(request);

    handle(&headers, &payload, &client, Some("_INBOX.err"), &nats, "session-001").await;

    assert_eq!(nats.published_messages(), vec!["_INBOX.err"]);
}

#[tokio::test]
async fn handle_invalid_payload_publishes_error_reply() {
    let nats = MockNatsClient::new();
    let client = MockClient::new(RequestPermissionOutcome::Cancelled);

    handle(
        &empty_headers(),
        b"not json",
        &client,
        Some("_INBOX.err"),
        &nats,
        "session-001",
    )
    .await;

    assert_eq!(nats.published_messages(), vec!["_INBOX.err"]);
}

#[tokio::test]
async fn handle_client_error_publishes_error_reply() {
    let nats = MockNatsClient::new();
    let client = FailingClient;
    let tool_call = ToolCallUpdate::new("call-1", ToolCallUpdateFields::new());
    let request = RequestPermissionRequest::new("session-001", tool_call, vec![]);
    let (headers, payload) = make_wire_request(request);

    handle(&headers, &payload, &client, Some("_INBOX.err"), &nats, "session-001").await;

    assert_eq!(nats.published_messages(), vec!["_INBOX.err"]);
}

#[tokio::test]
async fn handle_success_flush_failure_exercises_warn_path() {
    let nats = AdvancedMockNatsClient::new();
    nats.fail_next_flush();
    let client = MockClient::new(RequestPermissionOutcome::Cancelled);
    let tool_call = ToolCallUpdate::new("call-1", ToolCallUpdateFields::new());
    let request = RequestPermissionRequest::new("session-001", tool_call, vec![]);
    let (headers, payload) = make_wire_request(request);

    handle(&headers, &payload, &client, Some("_INBOX.reply"), &nats, "session-001").await;

    assert_eq!(nats.published_messages(), vec!["_INBOX.reply"]);
}

#[tokio::test]
async fn handle_success_publish_failure_exercises_error_path() {
    let nats = AdvancedMockNatsClient::new();
    nats.fail_next_publish();
    let client = MockClient::new(RequestPermissionOutcome::Cancelled);
    let tool_call = ToolCallUpdate::new("call-1", ToolCallUpdateFields::new());
    let request = RequestPermissionRequest::new("session-001", tool_call, vec![]);
    let (headers, payload) = make_wire_request(request);

    handle(&headers, &payload, &client, Some("_INBOX.reply"), &nats, "session-001").await;

    assert!(nats.published_messages().is_empty());
}
