use crate::jsonrpc::extract_request_id;
use crate::nats::{FlushClient, PublishClient, headers_with_trace_context};
use agent_client_protocol::{
    Client, Error, ErrorCode, ReadTextFileRequest, ReadTextFileResponse, Request, RequestId,
    Response,
};
use bytes::Bytes;
use serde::de::Error as SerdeDeError;
use tracing::{instrument, warn};
use trogon_std::JsonSerialize;

const CONTENT_TYPE_JSON: &str = "application/json";
const CONTENT_TYPE_PLAIN: &str = "text/plain";

fn error_response_fallback_bytes<S: JsonSerialize>(serializer: &S) -> (Bytes, &'static str) {
    match serializer.to_vec(&Response::<()>::Error {
        id: RequestId::Null,
        error: Error::new(-32603, "Internal error"),
    }) {
        Ok(v) => (Bytes::from(v), CONTENT_TYPE_JSON),
        Err(e) => {
            warn!(
                error = %e,
                "Fallback JSON serialization failed, response may not be valid JSON-RPC"
            );
            (Bytes::from("Internal error"), CONTENT_TYPE_PLAIN)
        }
    }
}

async fn publish_reply<N: PublishClient + FlushClient>(
    nats: &N,
    reply_to: &str,
    bytes: Bytes,
    content_type: &str,
    context: &str,
) {
    let mut headers = headers_with_trace_context();
    headers.insert("Content-Type", content_type);
    if let Err(e) = nats
        .publish_with_headers(reply_to.to_string(), headers, bytes)
        .await
    {
        warn!(error = %e, "Failed to publish {}", context);
    }
    if let Err(e) = nats.flush().await {
        warn!(error = %e, "Failed to flush {}", context);
    }
}

fn error_response_bytes<S: JsonSerialize>(
    serializer: &S,
    request_id: RequestId,
    code: ErrorCode,
    message: &str,
) -> (Bytes, &'static str) {
    let response = Response::<()>::Error {
        id: request_id,
        error: Error::new(i32::from(code), message),
    };
    match serializer.to_vec(&response) {
        Ok(v) => (Bytes::from(v), CONTENT_TYPE_JSON),
        Err(e) => {
            warn!(error = %e, "JSON serialization failed, using fallback error");
            error_response_fallback_bytes(serializer)
        }
    }
}

#[derive(Debug)]
pub enum FsReadTextFileError {
    InvalidRequest(serde_json::Error),
    ClientError(agent_client_protocol::Error),
}

impl std::fmt::Display for FsReadTextFileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidRequest(e) => write!(f, "invalid request: {}", e),
            Self::ClientError(e) => write!(f, "client error: {}", e),
        }
    }
}

impl std::error::Error for FsReadTextFileError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidRequest(e) => Some(e),
            Self::ClientError(e) => Some(e),
        }
    }
}

pub fn error_code_and_message(e: &FsReadTextFileError) -> (ErrorCode, String) {
    match e {
        FsReadTextFileError::InvalidRequest(inner) => (
            ErrorCode::InvalidParams,
            format!("Invalid read_text_file request: {}", inner),
        ),
        FsReadTextFileError::ClientError(inner) => (inner.code, inner.message.clone()),
    }
}

/// Handles read_text_file: parses request, calls client, wraps response in JSON-RPC envelope,
/// and publishes to reply subject. Reply is required (request-reply pattern).
#[instrument(
    name = "acp.client.fs.read_text_file",
    skip(payload, client, nats, serializer)
)]
pub async fn handle<N: PublishClient + FlushClient, C: Client, S: JsonSerialize>(
    payload: &[u8],
    client: &C,
    reply: Option<&str>,
    nats: &N,
    session_id: &str,
    serializer: &S,
) {
    let reply_to = match reply {
        Some(r) => r,
        None => {
            warn!(
                session_id = %session_id,
                "read_text_file requires reply subject; ignoring message"
            );
            return;
        }
    };

    let request_id = extract_request_id(payload);
    match forward_to_client(payload, client).await {
        Ok(response) => {
            let (response_bytes, content_type) = serializer
                .to_vec(&Response::Result {
                    id: request_id.clone(),
                    result: response,
                })
                .map(|v| (Bytes::from(v), CONTENT_TYPE_JSON))
                .unwrap_or_else(|e| {
                    warn!(error = %e, "JSON serialization of response failed, sending error reply");
                    error_response_bytes(
                        serializer,
                        request_id,
                        ErrorCode::InternalError,
                        &format!("Failed to serialize response: {}", e),
                    )
                });
            publish_reply(
                nats,
                reply_to,
                response_bytes,
                content_type,
                "fs_read_text_file reply",
            )
            .await;
        }
        Err(e) => {
            let (code, message) = error_code_and_message(&e);
            warn!(
                error = %e,
                session_id = %session_id,
                "Failed to handle fs_read_text_file"
            );
            let (bytes, content_type) =
                error_response_bytes(serializer, request_id, code, &message);
            publish_reply(
                nats,
                reply_to,
                bytes,
                content_type,
                "fs_read_text_file error reply",
            )
            .await;
        }
    }
}

async fn forward_to_client<C: Client>(
    payload: &[u8],
    client: &C,
) -> Result<ReadTextFileResponse, FsReadTextFileError> {
    let envelope: Request<ReadTextFileRequest> =
        serde_json::from_slice(payload).map_err(FsReadTextFileError::InvalidRequest)?;
    let request = envelope.params.ok_or_else(|| {
        FsReadTextFileError::InvalidRequest(serde_json::Error::custom("params is null or missing"))
    })?;
    client
        .read_text_file(request)
        .await
        .map_err(FsReadTextFileError::ClientError)
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::{
        ContentBlock, ContentChunk, ReadTextFileRequest, ReadTextFileResponse, Request, RequestId,
        RequestPermissionRequest, RequestPermissionResponse, SessionNotification, SessionUpdate,
    };
    use async_trait::async_trait;
    use std::error::Error;
    use trogon_nats::{AdvancedMockNatsClient, MockNatsClient};
    use trogon_std::{FailNextSerialize, StdJsonSerialize};

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
        async fn session_notification(
            &self,
            _: SessionNotification,
        ) -> agent_client_protocol::Result<()> {
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

        async fn read_text_file(
            &self,
            _: ReadTextFileRequest,
        ) -> agent_client_protocol::Result<ReadTextFileResponse> {
            Ok(ReadTextFileResponse::new(self.content.clone()))
        }
    }

    struct FailingClient;

    #[async_trait(?Send)]
    impl Client for FailingClient {
        async fn session_notification(
            &self,
            _: SessionNotification,
        ) -> agent_client_protocol::Result<()> {
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

        async fn read_text_file(
            &self,
            _: ReadTextFileRequest,
        ) -> agent_client_protocol::Result<ReadTextFileResponse> {
            Err(agent_client_protocol::Error::new(
                i32::from(ErrorCode::InvalidParams),
                "file not found",
            ))
        }
    }

    #[tokio::test]
    async fn fs_read_text_file_forwards_request_and_returns_response() {
        let client = MockClient::new("hello world");
        let request = ReadTextFileRequest::new(
            agent_client_protocol::SessionId::from("sess-1"),
            "/tmp/foo.txt".to_string(),
        );
        let envelope = Request {
            id: RequestId::Number(1),
            method: std::sync::Arc::from("fs/read_text_file"),
            params: Some(request),
        };
        let payload = serde_json::to_vec(&envelope).unwrap();

        let result = forward_to_client(&payload, &client).await;
        assert!(result.is_ok());
        let response = result.unwrap();
        assert_eq!(response.content, "hello world");
    }

    #[tokio::test]
    async fn fs_read_text_file_returns_error_when_payload_is_invalid_json() {
        let client = MockClient::new("hello");
        let result = forward_to_client(b"not json", &client).await;
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
        let envelope = Request {
            id: RequestId::Number(1),
            method: std::sync::Arc::from("fs/read_text_file"),
            params: Some(ReadTextFileRequest::new(
                agent_client_protocol::SessionId::from("sess-1"),
                "/missing.txt".to_string(),
            )),
        };
        let payload = serde_json::to_vec(&envelope).unwrap();

        let result = forward_to_client(&payload, &client).await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            FsReadTextFileError::ClientError(_)
        ));
    }

    #[tokio::test]
    async fn handle_success_serialization_fallback_sends_error_reply() {
        let nats = MockNatsClient::new();
        let client = MockClient::new("content");
        let serializer = FailNextSerialize::new(1);
        let envelope = Request {
            id: RequestId::Number(1),
            method: std::sync::Arc::from("fs/read_text_file"),
            params: Some(ReadTextFileRequest::new(
                agent_client_protocol::SessionId::from("sess-1"),
                "/path.txt".to_string(),
            )),
        };
        let payload = serde_json::to_vec(&envelope).unwrap();

        handle(
            &payload,
            &client,
            Some("_INBOX.reply"),
            &nats,
            "sess-1",
            &serializer,
        )
        .await;

        assert_eq!(nats.published_messages(), vec!["_INBOX.reply"]);
    }

    #[tokio::test]
    async fn handle_success_publishes_response_to_reply_subject() {
        let nats = MockNatsClient::new();
        let client = MockClient::new("file content");
        let envelope = Request {
            id: RequestId::Number(42),
            method: std::sync::Arc::from("fs/read_text_file"),
            params: Some(ReadTextFileRequest::new(
                agent_client_protocol::SessionId::from("sess-1"),
                "/path/to/file.txt".to_string(),
            )),
        };
        let payload = serde_json::to_vec(&envelope).unwrap();

        handle(
            &payload,
            &client,
            Some("_INBOX.reply"),
            &nats,
            "sess-1",
            &StdJsonSerialize,
        )
        .await;

        assert_eq!(nats.published_messages(), vec!["_INBOX.reply"]);
    }

    #[tokio::test]
    async fn handle_no_reply_does_not_publish() {
        let nats = MockNatsClient::new();
        let client = MockClient::new("content");
        let envelope = Request {
            id: RequestId::Number(1),
            method: std::sync::Arc::from("fs/read_text_file"),
            params: Some(ReadTextFileRequest::new(
                agent_client_protocol::SessionId::from("sess-1"),
                "/tmp/foo.txt".to_string(),
            )),
        };
        let payload = serde_json::to_vec(&envelope).unwrap();

        handle(&payload, &client, None, &nats, "sess-1", &StdJsonSerialize).await;

        assert!(nats.published_messages().is_empty());
    }

    #[tokio::test]
    async fn handle_invalid_payload_publishes_error_reply() {
        let nats = MockNatsClient::new();
        let client = MockClient::new("content");

        handle(
            b"not json",
            &client,
            Some("_INBOX.err"),
            &nats,
            "sess-1",
            &StdJsonSerialize,
        )
        .await;

        assert_eq!(nats.published_messages(), vec!["_INBOX.err"]);
    }

    #[tokio::test]
    async fn handle_client_error_serialization_last_resort_returns_plain_text() {
        let nats = MockNatsClient::new();
        let client = FailingClient;
        let serializer = FailNextSerialize::new(2);
        let envelope = Request {
            id: RequestId::Number(1),
            method: std::sync::Arc::from("fs/read_text_file"),
            params: Some(ReadTextFileRequest::new(
                agent_client_protocol::SessionId::from("sess-1"),
                "/missing.txt".to_string(),
            )),
        };
        let payload = serde_json::to_vec(&envelope).unwrap();

        handle(
            &payload,
            &client,
            Some("_INBOX.err"),
            &nats,
            "sess-1",
            &serializer,
        )
        .await;

        assert_eq!(nats.published_messages(), vec!["_INBOX.err"]);
    }

    #[tokio::test]
    async fn handle_client_error_publishes_error_reply() {
        let nats = MockNatsClient::new();
        let client = FailingClient;
        let envelope = Request {
            id: RequestId::Number(1),
            method: std::sync::Arc::from("fs/read_text_file"),
            params: Some(ReadTextFileRequest::new(
                agent_client_protocol::SessionId::from("sess-1"),
                "/missing.txt".to_string(),
            )),
        };
        let payload = serde_json::to_vec(&envelope).unwrap();

        handle(
            &payload,
            &client,
            Some("_INBOX.err"),
            &nats,
            "sess-1",
            &StdJsonSerialize,
        )
        .await;

        assert_eq!(nats.published_messages(), vec!["_INBOX.err"]);
    }

    #[test]
    fn error_code_and_message_invalid_request_returns_invalid_params() {
        let err = serde_json::from_slice::<ReadTextFileRequest>(b"not json").unwrap_err();
        let fs_err = FsReadTextFileError::InvalidRequest(err);
        let (code, message) = error_code_and_message(&fs_err);
        assert_eq!(code, ErrorCode::InvalidParams);
        assert!(message.contains("Invalid read_text_file request"));
    }

    #[test]
    fn error_code_and_message_client_error_preserves_client_code() {
        let client_err =
            agent_client_protocol::Error::new(ErrorCode::InvalidParams.into(), "file not found");
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
        let envelope = Request {
            id: RequestId::Number(1),
            method: std::sync::Arc::from("fs/read_text_file"),
            params: Some(ReadTextFileRequest::new(
                agent_client_protocol::SessionId::from("sess-1"),
                "/path/file.txt".to_string(),
            )),
        };
        let payload = serde_json::to_vec(&envelope).unwrap();

        handle(
            &payload,
            &client,
            Some("_INBOX.reply"),
            &nats,
            "sess-1",
            &StdJsonSerialize,
        )
        .await;

        assert_eq!(nats.published_messages(), vec!["_INBOX.reply"]);
    }

    #[tokio::test]
    async fn handle_success_publish_failure_exercises_error_path() {
        let nats = AdvancedMockNatsClient::new();
        nats.fail_next_publish();
        let client = MockClient::new("content");
        let envelope = Request {
            id: RequestId::Number(1),
            method: std::sync::Arc::from("fs/read_text_file"),
            params: Some(ReadTextFileRequest::new(
                agent_client_protocol::SessionId::from("sess-1"),
                "/path/file.txt".to_string(),
            )),
        };
        let payload = serde_json::to_vec(&envelope).unwrap();

        handle(
            &payload,
            &client,
            Some("_INBOX.reply"),
            &nats,
            "sess-1",
            &StdJsonSerialize,
        )
        .await;

        assert!(nats.published_messages().is_empty());
    }

    #[tokio::test]
    async fn handle_client_error_publish_failure_exercises_error_path() {
        let nats = AdvancedMockNatsClient::new();
        nats.fail_next_publish();
        let client = FailingClient;
        let envelope = Request {
            id: RequestId::Number(1),
            method: std::sync::Arc::from("fs/read_text_file"),
            params: Some(ReadTextFileRequest::new(
                agent_client_protocol::SessionId::from("sess-1"),
                "/missing.txt".to_string(),
            )),
        };
        let payload = serde_json::to_vec(&envelope).unwrap();

        handle(
            &payload,
            &client,
            Some("_INBOX.err"),
            &nats,
            "sess-1",
            &StdJsonSerialize,
        )
        .await;

        assert!(nats.published_messages().is_empty());
    }

    #[tokio::test]
    async fn forward_to_client_params_none_returns_invalid_request() {
        let client = MockClient::new("content");
        let envelope = Request::<ReadTextFileRequest> {
            id: RequestId::Number(1),
            method: std::sync::Arc::from("fs/read_text_file"),
            params: None,
        };
        let payload = serde_json::to_vec(&envelope).unwrap();

        let result = forward_to_client(&payload, &client).await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            FsReadTextFileError::InvalidRequest(_)
        ));
    }

    #[test]
    fn fs_read_text_file_error_display() {
        let err = serde_json::from_slice::<ReadTextFileRequest>(b"not json").unwrap_err();
        let fs_err = FsReadTextFileError::InvalidRequest(err);
        assert!(fs_err.to_string().contains("invalid request"));

        let client_err =
            agent_client_protocol::Error::new(ErrorCode::InvalidParams.into(), "file not found");
        let fs_err = FsReadTextFileError::ClientError(client_err);
        assert!(fs_err.to_string().contains("client error"));
    }

    #[test]
    fn fs_read_text_file_error_source() {
        let err = serde_json::from_slice::<ReadTextFileRequest>(b"not json").unwrap_err();
        let fs_err = FsReadTextFileError::InvalidRequest(err);
        assert!(fs_err.source().is_some());

        let client_err =
            agent_client_protocol::Error::new(ErrorCode::InvalidParams.into(), "file not found");
        let fs_err = FsReadTextFileError::ClientError(client_err);
        assert!(fs_err.source().is_some());
    }

    #[test]
    fn error_response_bytes_first_fallback_uses_null_id() {
        let mock = FailNextSerialize::new(1);
        let (bytes, content_type) = error_response_bytes(
            &mock,
            RequestId::Number(42),
            ErrorCode::InvalidParams,
            "test message",
        );
        assert_eq!(content_type, "application/json");
        let parsed: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(parsed["id"], serde_json::Value::Null);
        assert_eq!(parsed["error"]["code"], -32603);
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

    #[test]
    fn error_response_bytes_last_resort_returns_plain_text() {
        let mock = FailNextSerialize::new(2);
        let (bytes, content_type) =
            error_response_bytes(&mock, RequestId::Number(1), ErrorCode::InternalError, "msg");
        assert_eq!(content_type, "text/plain");
        assert_eq!(bytes.as_ref(), b"Internal error");
    }

    #[test]
    fn error_response_fallback_bytes_std_serializer_returns_json() {
        let (bytes, content_type) = error_response_fallback_bytes(&StdJsonSerialize);
        assert_eq!(content_type, "application/json");
        let parsed: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(parsed["id"], serde_json::Value::Null);
        assert_eq!(parsed["error"]["code"], -32603);
    }
}
