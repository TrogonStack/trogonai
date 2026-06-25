use super::*;
use agent_client_protocol::{
    ContentBlock, ContentChunk, ReadTextFileRequest, ReadTextFileResponse, RequestPermissionRequest,
    RequestPermissionResponse, SessionNotification, SessionUpdate,
};
use async_nats::header::HeaderMap;
use async_trait::async_trait;
use jsonrpc_nats::RequestId;
use std::error::Error;
use trogon_nats::{AdvancedMockNatsClient, MockNatsClient};

fn empty_headers() -> HeaderMap {
    HeaderMap::new()
}

fn make_wire_request<T: serde::Serialize>(params: &T) -> (HeaderMap, Vec<u8>) {
    crate::client::test_support::encode_wire_request("fs/read_text_file", RequestId::Number(1), params)
}

fn sample_request() -> ReadTextFileRequest {
    ReadTextFileRequest::new(
        agent_client_protocol::SessionId::from("sess-1"),
        "/tmp/foo.txt".to_string(),
    )
}
struct MockClient {
    content: String,
}

impl MockClient {
    fn new(content: &str) -> Self {
        Self {
            content: content.to_string(),
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

    async fn read_text_file(&self, _: ReadTextFileRequest) -> agent_client_protocol::Result<ReadTextFileResponse> {
        Ok(ReadTextFileResponse::new(self.content.clone()))
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

    async fn read_text_file(&self, _: ReadTextFileRequest) -> agent_client_protocol::Result<ReadTextFileResponse> {
        Err(agent_client_protocol::Error::new(
            i32::from(ErrorCode::InvalidParams),
            "file not found",
        ))
    }
}

#[tokio::test]
async fn fs_read_text_file_forwards_request_and_returns_response() {
    let client = MockClient::new("hello world");
    let (headers, payload) = make_wire_request(&ReadTextFileRequest::new(
        agent_client_protocol::SessionId::from("sess-1"),
        "/tmp/foo.txt".to_string(),
    ));

    let result = forward_to_client(&headers, &payload, &client).await;
    assert!(result.is_ok());
    let response = result.unwrap();
    assert_eq!(response.content, "hello world");
}

#[tokio::test]
async fn fs_read_text_file_returns_error_when_payload_is_invalid_json() {
    let client = MockClient::new("hello");
    let result = forward_to_client(&empty_headers(), b"not json", &client).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn mock_client_session_notification_returns_ok() {
    let client = MockClient::new("x");
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
async fn fs_read_text_file_returns_client_error_when_client_fails() {
    let client = FailingClient;
    let request = sample_request();
    let (headers, payload) = make_wire_request(&request);

    let result = forward_to_client(&headers, &payload, &client).await;
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), FsReadTextFileError::ClientError(_)));
}

#[tokio::test]
async fn handle_success_publishes_response_to_reply_subject() {
    let nats = MockNatsClient::new();
    let client = MockClient::new("file content");
    let request = sample_request();
    let (headers, payload) = make_wire_request(&request);

    handle(&headers, &payload, &client, Some("_INBOX.reply"), &nats, "sess-1").await;

    assert_eq!(nats.published_messages(), vec!["_INBOX.reply"]);
}

#[tokio::test]
async fn handle_no_reply_does_not_publish() {
    let nats = MockNatsClient::new();
    let client = MockClient::new("content");
    let request = sample_request();
    let (headers, payload) = make_wire_request(&request);

    handle(&headers, &payload, &client, None, &nats, "sess-1").await;

    assert!(nats.published_messages().is_empty());
}

#[tokio::test]
async fn handle_invalid_payload_publishes_error_reply() {
    let nats = MockNatsClient::new();
    let client = MockClient::new("content");

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
    let request = sample_request();
    let (headers, payload) = make_wire_request(&request);

    handle(&headers, &payload, &client, Some("_INBOX.err"), &nats, "sess-1").await;

    assert_eq!(nats.published_messages(), vec!["_INBOX.err"]);
}

#[test]
fn error_code_and_message_invalid_request_returns_invalid_params() {
    let err = "Invalid read_text_file request: invalid request".to_string();
    let fs_err = FsReadTextFileError::InvalidRequest(err);
    let (code, message) = error_code_and_message(&fs_err);
    assert_eq!(code, ErrorCode::InvalidParams);
    assert!(message.contains("Invalid read_text_file request"));
}

#[test]
fn error_code_and_message_client_error_preserves_client_code() {
    let client_err = agent_client_protocol::Error::new(ErrorCode::InvalidParams.into(), "file not found");
    let fs_err = FsReadTextFileError::ClientError(client_err);
    let (code, message) = error_code_and_message(&fs_err);
    assert_eq!(code, ErrorCode::InvalidParams);
    assert_eq!(message, "file not found");
}

#[tokio::test]
async fn handle_success_flush_failure_exercises_warn_path() {
    let nats = AdvancedMockNatsClient::new();
    nats.fail_next_flush();
    let client = MockClient::new("content");
    let request = sample_request();
    let (headers, payload) = make_wire_request(&request);

    handle(&headers, &payload, &client, Some("_INBOX.reply"), &nats, "sess-1").await;

    assert_eq!(nats.published_messages(), vec!["_INBOX.reply"]);
}

#[tokio::test]
async fn handle_success_publish_failure_exercises_error_path() {
    let nats = AdvancedMockNatsClient::new();
    nats.fail_next_publish();
    let client = MockClient::new("content");
    let request = sample_request();
    let (headers, payload) = make_wire_request(&request);

    handle(&headers, &payload, &client, Some("_INBOX.reply"), &nats, "sess-1").await;

    assert!(nats.published_messages().is_empty());
}

#[tokio::test]
async fn handle_client_error_publish_failure_exercises_error_path() {
    let nats = AdvancedMockNatsClient::new();
    nats.fail_next_publish();
    let client = FailingClient;
    let request = sample_request();
    let (headers, payload) = make_wire_request(&request);

    handle(&headers, &payload, &client, Some("_INBOX.err"), &nats, "sess-1").await;

    assert!(nats.published_messages().is_empty());
}

#[tokio::test]
async fn forward_to_client_params_none_returns_invalid_request() {
    let client = MockClient::new("content");
    let result = forward_to_client(&empty_headers(), b"{}", &client).await;
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), FsReadTextFileError::InvalidRequest(_)));
}

#[test]
fn fs_read_text_file_error_display() {
    let err = "invalid request".to_string();
    let expected = "invalid request: invalid request";
    let fs_err = FsReadTextFileError::InvalidRequest(err.to_string());
    assert_eq!(fs_err.to_string(), expected);

    let client_err = agent_client_protocol::Error::new(ErrorCode::InvalidParams.into(), "file not found");
    let fs_err = FsReadTextFileError::ClientError(client_err);
    assert_eq!(fs_err.to_string(), "client error: file not found");
}

#[test]
fn fs_read_text_file_error_source() {
    let err = "invalid request".to_string();
    let fs_err = FsReadTextFileError::InvalidRequest(err.to_string());
    assert!(fs_err.source().is_none());

    let client_err = agent_client_protocol::Error::new(ErrorCode::InvalidParams.into(), "file not found");
    let fs_err = FsReadTextFileError::ClientError(client_err);
    assert!(fs_err.source().is_some());
}

#[tokio::test]
async fn mock_client_request_permission_returns_err() {
    let client = MockClient::new("x");
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
