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
    crate::client::test_support::encode_wire_request("fs/write_text_file", RequestId::Number(1), params)
}

fn sample_request() -> WriteTextFileRequest {
    WriteTextFileRequest::new(
        agent_client_protocol::SessionId::from("sess-1"),
        "/tmp/foo.txt".to_string(),
        "data".to_string(),
    )
}
struct MockClient;

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
        Err(agent_client_protocol::Error::new(
            -32603,
            "not implemented in test mock",
        ))
    }

    async fn write_text_file(&self, _: WriteTextFileRequest) -> agent_client_protocol::Result<WriteTextFileResponse> {
        Ok(WriteTextFileResponse::new())
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
            -32603,
            "not implemented in test mock",
        ))
    }

    async fn write_text_file(&self, _: WriteTextFileRequest) -> agent_client_protocol::Result<WriteTextFileResponse> {
        Err(agent_client_protocol::Error::new(
            i32::from(ErrorCode::InvalidParams),
            "permission denied",
        ))
    }
}

#[tokio::test]
async fn fs_write_text_file_forwards_request_and_returns_response() {
    let client = MockClient;
    let request = sample_request();
    let (headers, payload) = make_wire_request(&request);

    let result = forward_to_client(&headers, &payload, &client, "sess-1").await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn fs_write_text_file_returns_error_when_payload_is_invalid_json() {
    let client = MockClient;
    let result = forward_to_client(&empty_headers(), b"not json", &client, "sess-1").await;
    assert!(result.is_err());
}

#[tokio::test]
async fn mock_client_session_notification_returns_ok() {
    let client = MockClient;
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
async fn fs_write_text_file_returns_client_error_when_client_fails() {
    let client = FailingClient;
    let request = sample_request();
    let (headers, payload) = make_wire_request(&request);

    let result = forward_to_client(&headers, &payload, &client, "sess-1").await;
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), FsWriteTextFileError::ClientError(_)));
}

#[tokio::test]
async fn handle_success_publishes_response_to_reply_subject() {
    let nats = MockNatsClient::new();
    let client = MockClient;
    let request = sample_request();
    let (headers, payload) = make_wire_request(&request);

    handle(&headers, &payload, &client, Some("_INBOX.reply"), &nats, "sess-1").await;

    assert_eq!(nats.published_messages(), vec!["_INBOX.reply"]);
}

#[tokio::test]
async fn handle_no_reply_does_not_publish() {
    let nats = MockNatsClient::new();
    let client = MockClient;
    let request = sample_request();
    let (headers, payload) = make_wire_request(&request);

    handle(&headers, &payload, &client, None, &nats, "sess-1").await;

    assert!(nats.published_messages().is_empty());
}

#[tokio::test]
async fn handle_client_error_publishes_error_reply_with_matching_id() {
    let nats = MockNatsClient::new();
    let client = FailingClient;
    let request = sample_request();
    let (headers, payload) = make_wire_request(&request);

    handle(&headers, &payload, &client, Some("_INBOX.err"), &nats, "sess-1").await;

    assert_eq!(nats.published_messages(), vec!["_INBOX.err"]);
    let published_headers = nats.published_headers()[0].clone();
    assert_eq!(
        published_headers.get(jsonrpc_nats::HEADER_ID).unwrap().as_str(),
        "1",
        "error response must preserve request id"
    );
    assert_eq!(
        published_headers.get(jsonrpc_nats::HEADER_ERROR_CODE).unwrap().as_str(),
        i32::from(ErrorCode::InvalidParams).to_string().as_str(),
        "error response must have error code"
    );
}

#[tokio::test]
async fn handle_invalid_payload_publishes_error_reply() {
    let nats = MockNatsClient::new();
    let client = MockClient;

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
    let published_headers = nats.published_headers()[0].clone();
    assert!(published_headers.get(jsonrpc_nats::HEADER_ERROR_CODE).is_some());
}

#[test]
fn error_code_and_message_invalid_request_returns_invalid_params() {
    let err = serde_json::from_slice::<WriteTextFileRequest>(b"not json").unwrap_err();
    let fs_err = FsWriteTextFileError::InvalidRequest(err);
    let (code, message) = error_code_and_message(&fs_err);
    assert_eq!(code, ErrorCode::InvalidParams);
    assert!(message.contains("Invalid write_text_file request"));
}

#[test]
fn error_code_and_message_client_error_preserves_client_code() {
    let client_err = agent_client_protocol::Error::new(ErrorCode::InvalidParams.into(), "permission denied");
    let fs_err = FsWriteTextFileError::ClientError(client_err);
    let (code, message) = error_code_and_message(&fs_err);
    assert_eq!(code, ErrorCode::InvalidParams);
    assert_eq!(message, "permission denied");
}

#[tokio::test]
async fn handle_success_flush_failure_exercises_warn_path() {
    let nats = AdvancedMockNatsClient::new();
    nats.fail_next_flush();
    let client = MockClient;
    let request = sample_request();
    let (headers, payload) = make_wire_request(&request);

    handle(&headers, &payload, &client, Some("_INBOX.reply"), &nats, "sess-1").await;

    assert_eq!(nats.published_messages(), vec!["_INBOX.reply"]);
}

#[tokio::test]
async fn handle_success_publish_failure_exercises_error_path() {
    let nats = AdvancedMockNatsClient::new();
    nats.fail_next_publish();
    let client = MockClient;
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
    let client = MockClient;
    let headers = empty_headers();
    let payload = b"{}";

    let result = forward_to_client(&headers, payload, &client, "sess-1").await;
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), FsWriteTextFileError::InvalidRequest(_)));
}

#[tokio::test]
async fn forward_to_client_session_id_mismatch_returns_invalid_request() {
    let client = MockClient;
    let request = sample_request();
    let (headers, payload) = make_wire_request(&request);

    let result = forward_to_client(&headers, &payload, &client, "sess-other").await;
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), FsWriteTextFileError::InvalidRequest(_)));
}

#[test]
fn fs_write_text_file_error_display() {
    let err = serde_json::from_slice::<WriteTextFileRequest>(b"not json").unwrap_err();
    let expected = format!("invalid request: {err}");
    let fs_err = FsWriteTextFileError::InvalidRequest(err);
    assert_eq!(fs_err.to_string(), expected);

    let client_err = agent_client_protocol::Error::new(ErrorCode::InvalidParams.into(), "permission denied");
    let fs_err = FsWriteTextFileError::ClientError(client_err);
    assert_eq!(fs_err.to_string(), "client error: permission denied");
}

#[test]
fn fs_write_text_file_error_source() {
    let err = serde_json::from_slice::<WriteTextFileRequest>(b"not json").unwrap_err();
    let fs_err = FsWriteTextFileError::InvalidRequest(err);
    assert!(fs_err.source().is_some());

    let client_err = agent_client_protocol::Error::new(ErrorCode::InvalidParams.into(), "permission denied");
    let fs_err = FsWriteTextFileError::ClientError(client_err);
    assert!(fs_err.source().is_some());
}

#[tokio::test]
async fn mock_client_request_permission_returns_err() {
    let client = MockClient;
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
async fn mock_client_read_text_file_returns_err() {
    let client = MockClient;
    let req = ReadTextFileRequest::new(
        agent_client_protocol::SessionId::from("sess-1"),
        "/tmp/file.txt".to_string(),
    );
    let result = client.read_text_file(req).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn failing_client_read_text_file_returns_err() {
    let client = FailingClient;
    let req = ReadTextFileRequest::new(
        agent_client_protocol::SessionId::from("sess-1"),
        "/tmp/file.txt".to_string(),
    );
    let result = client.read_text_file(req).await;
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
