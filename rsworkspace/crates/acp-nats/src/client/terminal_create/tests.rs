use super::*;
use agent_client_protocol::{
    ContentBlock, ContentChunk, CreateTerminalRequest, CreateTerminalResponse,
    RequestPermissionRequest, RequestPermissionResponse, SessionNotification, SessionUpdate,
};
use async_trait::async_trait;
use std::error::Error;
use trogon_nats::MockNatsClient;
use async_nats::header::HeaderMap;
use jsonrpc_nats::RequestId;

fn empty_headers() -> HeaderMap {
    HeaderMap::new()
}

fn make_wire_request<T: serde::Serialize>(params: &T) -> (HeaderMap, Vec<u8>) {
    crate::client::test_support::encode_wire_request(
        "terminal/create",
        RequestId::Number(1),
        params,
    )
}


fn sample_request() -> CreateTerminalRequest {
    CreateTerminalRequest::new("sess-1", "echo")
}
struct MockClient {
    terminal_id: String,
}

impl MockClient {
    fn new(terminal_id: &str) -> Self {
        Self {
            terminal_id: terminal_id.to_string(),
        }
    }
}

#[async_trait(?Send)]
impl Client for MockClient {
    async fn session_notification(&self, _: SessionNotification) -> agent_client_protocol::Result<()> {
        Ok(())
    }

    async fn request_permission(
        &self,
        _: RequestPermissionRequest,
    ) -> agent_client_protocol::Result<RequestPermissionResponse> {
        Err(agent_client_protocol::Error::new(
            -32603,
            "not implemented in test mock",
        ))
    }

    async fn create_terminal(&self, _: CreateTerminalRequest) -> agent_client_protocol::Result<CreateTerminalResponse> {
        Ok(CreateTerminalResponse::new(self.terminal_id.clone()))
    }
}

struct FailingClient;

#[async_trait(?Send)]
impl Client for FailingClient {
    async fn session_notification(&self, _: SessionNotification) -> agent_client_protocol::Result<()> {
        Ok(())
    }

    async fn request_permission(
        &self,
        _: RequestPermissionRequest,
    ) -> agent_client_protocol::Result<RequestPermissionResponse> {
        Err(agent_client_protocol::Error::new(
            -32603,
            "not implemented in test mock",
        ))
    }

    async fn create_terminal(&self, _: CreateTerminalRequest) -> agent_client_protocol::Result<CreateTerminalResponse> {
        Err(agent_client_protocol::Error::new(
            -32603,
            "mock create_terminal failure",
        ))
    }
}

#[tokio::test]
async fn handle_success_publishes_response_to_reply_subject() {
    let nats = MockNatsClient::new();
    let client = MockClient::new("term-001");
    let request = CreateTerminalRequest::new("sess-1", "echo hello");
    let (headers, payload) = make_wire_request(&request);

    handle(
        &headers,
        &payload,
        &client,
        Some("_INBOX.reply"),
        &nats,
        "sess-1",
    )
    .await;

    assert_eq!(nats.published_messages(), vec!["_INBOX.reply"]);
}

#[tokio::test]
async fn handle_no_reply_does_not_publish() {
    let nats = MockNatsClient::new();
    let client = MockClient::new("term-001");
    let request = CreateTerminalRequest::new("sess-1", "echo hello");
    let (headers, payload) = make_wire_request(&request);

    handle(
        &headers,
        &payload, &client, None, &nats, "sess-1").await;

    assert!(nats.published_messages().is_empty());
}

#[tokio::test]
async fn handle_invalid_payload_publishes_error_reply() {
    let nats = MockNatsClient::new();
    let client = MockClient::new("term-001");

    handle(
        &empty_headers(),
        b"not json",
        &client,
        Some("_INBOX.err"),
        &nats,
        "sess-1",
    )
    .await;

    assert_eq!(nats.published_messages(), vec!["_INBOX.err"]);
}

#[tokio::test]
async fn handle_client_error_publishes_error_reply() {
    let nats = MockNatsClient::new();
    let client = FailingClient;
    let request = CreateTerminalRequest::new("sess-1", "echo hello");
    let (headers, payload) = make_wire_request(&request);

    handle(
        &headers,
        &payload,
        &client,
        Some("_INBOX.err"),
        &nats,
        "sess-1",
    )
    .await;

    assert_eq!(nats.published_messages(), vec!["_INBOX.err"]);
}

#[test]
fn error_code_and_message_invalid_request_returns_invalid_params() {
    let err = serde_json::from_slice::<CreateTerminalRequest>(b"not json").unwrap_err();
    let tc_err = TerminalCreateError::InvalidRequest(err);
    let (code, message) = error_code_and_message(&tc_err);
    assert_eq!(code, ErrorCode::InvalidParams);
    assert!(message.contains("Invalid terminal/create request"));
}

#[test]
fn error_code_and_message_client_error_preserves_client_code() {
    let client_err = agent_client_protocol::Error::new(ErrorCode::InvalidParams.into(), "file not found");
    let tc_err = TerminalCreateError::ClientError(client_err);
    let (code, message) = error_code_and_message(&tc_err);
    assert_eq!(code, ErrorCode::InvalidParams);
    assert_eq!(message, "file not found");
}

#[test]
fn error_code_and_message_client_error_preserves_other_code() {
    let client_err = agent_client_protocol::Error::new(ErrorCode::InternalError.into(), "internal");
    let tc_err = TerminalCreateError::ClientError(client_err);
    let (code, _) = error_code_and_message(&tc_err);
    assert_eq!(code, ErrorCode::InternalError);
}

#[tokio::test]
async fn forward_to_client_params_none_returns_invalid_request() {
    let client = MockClient::new("term-001");
    let request = sample_request();
    let (headers, payload) = make_wire_request(&request);
    let headers = empty_headers();
    let payload = b"{}";

    let result = forward_to_client(&headers, &payload, &client, "sess-1").await;
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), TerminalCreateError::InvalidRequest(_)));
}

#[tokio::test]
async fn forward_to_client_session_id_mismatch_returns_invalid_request() {
    let client = MockClient::new("term-001");
    let request = CreateTerminalRequest::new("sess-b", "echo hello");
    

    let (headers, payload) = make_wire_request(&CreateTerminalRequest::new("sess-b", "echo hello"));

    let result = forward_to_client(&headers, &payload, &client, "sess-a").await;
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), TerminalCreateError::InvalidRequest(_)));
}

#[tokio::test]
async fn handle_session_id_mismatch_publishes_error_reply() {
    let nats = MockNatsClient::new();
    let client = MockClient::new("term-001");
    let request = CreateTerminalRequest::new("sess-b", "echo hello");
    let (headers, payload) = make_wire_request(&request);

    handle(
        &headers,
        &payload,
        &client,
        Some("_INBOX.err"),
        &nats,
        "sess-a",
    )
    .await;

    assert_eq!(nats.published_messages(), vec!["_INBOX.err"]);
}


#[test]
fn terminal_create_error_display() {
    let err = serde_json::from_slice::<CreateTerminalRequest>(b"not json").unwrap_err();
    let expected = format!("invalid request: {err}");
    let tc_err = TerminalCreateError::InvalidRequest(err);
    assert_eq!(tc_err.to_string(), expected);

    let client_err = agent_client_protocol::Error::new(ErrorCode::InvalidParams.into(), "create failed");
    let tc_err = TerminalCreateError::ClientError(client_err);
    assert_eq!(tc_err.to_string(), "client error: create failed");
}

#[test]
fn terminal_create_error_source() {
    let err = serde_json::from_slice::<CreateTerminalRequest>(b"not json").unwrap_err();
    let tc_err = TerminalCreateError::InvalidRequest(err);
    assert!(tc_err.source().is_some());

    let client_err = agent_client_protocol::Error::new(ErrorCode::InvalidParams.into(), "create failed");
    let tc_err = TerminalCreateError::ClientError(client_err);
    assert!(tc_err.source().is_some());
}

#[tokio::test]
async fn mock_client_session_notification_returns_ok() {
    let client = MockClient::new("term-001");
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
async fn mock_client_request_permission_returns_err() {
    let client = MockClient::new("term-001");
    let req: RequestPermissionRequest = serde_json::from_value(serde_json::json!({
        "sessionId": "sess-1",
        "toolCall": { "toolCallId": "call-1" },
        "options": []
    }))
    .unwrap();
    let result = client.request_permission(req).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn failing_client_request_permission_returns_err() {
    let client = FailingClient;
    let req: RequestPermissionRequest = serde_json::from_value(serde_json::json!({
        "sessionId": "sess-1",
        "toolCall": { "toolCallId": "call-1" },
        "options": []
    }))
    .unwrap();
    let result = client.request_permission(req).await;
    assert!(result.is_err());
}
