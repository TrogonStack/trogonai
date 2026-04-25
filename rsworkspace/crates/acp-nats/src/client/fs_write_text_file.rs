use crate::client::rpc_reply;
use crate::jsonrpc::extract_request_id;
use crate::nats::{FlushClient, PublishClient};
use agent_client_protocol::{Client, ErrorCode, Request, Response, WriteTextFileRequest, WriteTextFileResponse};
use bytes::Bytes;
use serde::de::Error as SerdeDeError;
use tracing::{instrument, warn};
use trogon_std::JsonSerialize;

#[derive(Debug)]
pub enum FsWriteTextFileError {
    InvalidRequest(serde_json::Error),
    ClientError(agent_client_protocol::Error),
}

impl std::fmt::Display for FsWriteTextFileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidRequest(e) => write!(f, "invalid request: {}", e),
            Self::ClientError(e) => write!(f, "client error: {}", e),
        }
    }
}

impl std::error::Error for FsWriteTextFileError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidRequest(e) => Some(e),
            Self::ClientError(e) => Some(e),
        }
    }
}

pub fn error_code_and_message(e: &FsWriteTextFileError) -> (ErrorCode, String) {
    match e {
        FsWriteTextFileError::InvalidRequest(inner) => (
            ErrorCode::InvalidParams,
            format!("Invalid write_text_file request: {}", inner),
        ),
        FsWriteTextFileError::ClientError(inner) => (inner.code, inner.message.clone()),
    }
}

/// Handles write_text_file: parses request, calls client, wraps response in JSON-RPC envelope,
/// and publishes to reply subject. Reply is required (request-reply pattern).
#[instrument(name = "acp.client.fs.write_text_file", skip(payload, client, nats, serializer))]
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
                "write_text_file requires reply subject; ignoring message"
            );
            return;
        }
    };

    let request_id = extract_request_id(payload);
    match forward_to_client(payload, client, session_id).await {
        Ok(response) => {
            let (response_bytes, content_type) = serializer
                .to_vec(&Response::Result {
                    id: request_id.clone(),
                    result: response,
                })
                .map(|v| (Bytes::from(v), rpc_reply::CONTENT_TYPE_JSON))
                .unwrap_or_else(|e| {
                    warn!(error = %e, "JSON serialization of response failed, sending error reply");
                    rpc_reply::error_response_bytes(
                        serializer,
                        request_id,
                        ErrorCode::InternalError,
                        &format!("Failed to serialize response: {}", e),
                    )
                });
            rpc_reply::publish_reply(nats, reply_to, response_bytes, content_type, "fs_write_text_file reply").await;
        }
        Err(e) => {
            let (code, message) = error_code_and_message(&e);
            warn!(
                error = %e,
                session_id = %session_id,
                "Failed to handle fs_write_text_file"
            );
            let (bytes, content_type) = rpc_reply::error_response_bytes(serializer, request_id, code, &message);
            rpc_reply::publish_reply(nats, reply_to, bytes, content_type, "fs_write_text_file error reply").await;
        }
    }
}

async fn forward_to_client<C: Client>(
    payload: &[u8],
    client: &C,
    expected_session_id: &str,
) -> Result<WriteTextFileResponse, FsWriteTextFileError> {
    let envelope: Request<WriteTextFileRequest> =
        serde_json::from_slice(payload).map_err(FsWriteTextFileError::InvalidRequest)?;
    let request = envelope
        .params
        .ok_or_else(|| FsWriteTextFileError::InvalidRequest(serde_json::Error::custom("params is null or missing")))?;
    let params_session_id = request.session_id.to_string();
    if params_session_id != expected_session_id {
        return Err(FsWriteTextFileError::InvalidRequest(serde_json::Error::custom(
            format!(
                "params.sessionId ({}) does not match subject session id ({})",
                params_session_id, expected_session_id
            ),
        )));
    }
    client
        .write_text_file(request)
        .await
        .map_err(FsWriteTextFileError::ClientError)
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

        async fn write_text_file(
            &self,
            _: WriteTextFileRequest,
        ) -> agent_client_protocol::Result<WriteTextFileResponse> {
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

        async fn write_text_file(
            &self,
            _: WriteTextFileRequest,
        ) -> agent_client_protocol::Result<WriteTextFileResponse> {
            Err(agent_client_protocol::Error::new(
                i32::from(ErrorCode::InvalidParams),
                "permission denied",
            ))
        }
    }

    #[tokio::test]
    async fn fs_write_text_file_forwards_request_and_returns_response() {
        let client = MockClient;
        let envelope = Request {
            id: RequestId::Number(1),
            method: std::sync::Arc::from("fs/write_text_file"),
            params: Some(WriteTextFileRequest::new(
                agent_client_protocol::SessionId::from("sess-1"),
                "/tmp/foo.txt".to_string(),
                "content".to_string(),
            )),
        };
        let payload = serde_json::to_vec(&envelope).unwrap();

        let result = forward_to_client(&payload, &client, "sess-1").await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn fs_write_text_file_returns_error_when_payload_is_invalid_json() {
        let client = MockClient;
        let result = forward_to_client(b"not json", &client, "sess-1").await;
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
        let envelope = Request {
            id: RequestId::Number(1),
            method: std::sync::Arc::from("fs/write_text_file"),
            params: Some(WriteTextFileRequest::new(
                agent_client_protocol::SessionId::from("sess-1"),
                "/forbidden.txt".to_string(),
                "content".to_string(),
            )),
        };
        let payload = serde_json::to_vec(&envelope).unwrap();

        let result = forward_to_client(&payload, &client, "sess-1").await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), FsWriteTextFileError::ClientError(_)));
    }

    #[tokio::test]
    async fn handle_success_serialization_fallback_sends_error_reply() {
        let nats = MockNatsClient::new();
        let client = MockClient;
        let serializer = FailNextSerialize::new(1);
        let envelope = Request {
            id: RequestId::Number(1),
            method: std::sync::Arc::from("fs/write_text_file"),
            params: Some(WriteTextFileRequest::new(
                agent_client_protocol::SessionId::from("sess-1"),
                "/tmp/file.txt".to_string(),
                "content".to_string(),
            )),
        };
        let payload = serde_json::to_vec(&envelope).unwrap();

        handle(&payload, &client, Some("_INBOX.reply"), &nats, "sess-1", &serializer).await;

        assert_eq!(nats.published_messages(), vec!["_INBOX.reply"]);
    }

    #[tokio::test]
    async fn handle_success_publishes_response_to_reply_subject() {
        let nats = MockNatsClient::new();
        let client = MockClient;
        let envelope = Request {
            id: RequestId::Number(42),
            method: std::sync::Arc::from("fs/write_text_file"),
            params: Some(WriteTextFileRequest::new(
                agent_client_protocol::SessionId::from("sess-1"),
                "/tmp/file.txt".to_string(),
                "content".to_string(),
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
        let client = MockClient;
        let envelope = Request {
            id: RequestId::Number(1),
            method: std::sync::Arc::from("fs/write_text_file"),
            params: Some(WriteTextFileRequest::new(
                agent_client_protocol::SessionId::from("sess-1"),
                "/tmp/foo.txt".to_string(),
                "content".to_string(),
            )),
        };
        let payload = serde_json::to_vec(&envelope).unwrap();

        handle(&payload, &client, None, &nats, "sess-1", &StdJsonSerialize).await;

        assert!(nats.published_messages().is_empty());
    }

    #[tokio::test]
    async fn handle_client_error_publishes_error_reply_with_matching_id() {
        let nats = MockNatsClient::new();
        let client = FailingClient;
        let envelope = Request {
            id: RequestId::Number(99),
            method: std::sync::Arc::from("fs/write_text_file"),
            params: Some(WriteTextFileRequest::new(
                agent_client_protocol::SessionId::from("sess-1"),
                "/forbidden.txt".to_string(),
                "content".to_string(),
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
        let payloads = nats.published_payloads();
        assert_eq!(payloads.len(), 1);
        let response: serde_json::Value = serde_json::from_slice(payloads[0].as_ref()).unwrap();
        assert_eq!(response["id"], 99, "error response must preserve request id");
        assert!(response.get("error").is_some(), "error response must have error field");
        assert_eq!(response["error"]["code"], i32::from(ErrorCode::InvalidParams));
    }

    #[tokio::test]
    async fn handle_invalid_payload_publishes_error_reply() {
        let nats = MockNatsClient::new();
        let client = MockClient;

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
        let payloads = nats.published_payloads();
        assert_eq!(payloads.len(), 1);
        let response: serde_json::Value = serde_json::from_slice(payloads[0].as_ref()).unwrap();
        assert!(response.get("error").is_some());
    }

    #[tokio::test]
    async fn handle_client_error_serialization_last_resort_returns_plain_text() {
        let nats = MockNatsClient::new();
        let client = FailingClient;
        let serializer = FailNextSerialize::new(2);
        let envelope = Request {
            id: RequestId::Number(1),
            method: std::sync::Arc::from("fs/write_text_file"),
            params: Some(WriteTextFileRequest::new(
                agent_client_protocol::SessionId::from("sess-1"),
                "/forbidden.txt".to_string(),
                "content".to_string(),
            )),
        };
        let payload = serde_json::to_vec(&envelope).unwrap();

        handle(&payload, &client, Some("_INBOX.err"), &nats, "sess-1", &serializer).await;

        assert_eq!(nats.published_messages(), vec!["_INBOX.err"]);
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
        let envelope = Request {
            id: RequestId::Number(1),
            method: std::sync::Arc::from("fs/write_text_file"),
            params: Some(WriteTextFileRequest::new(
                agent_client_protocol::SessionId::from("sess-1"),
                "/tmp/file.txt".to_string(),
                "content".to_string(),
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
        let client = MockClient;
        let envelope = Request {
            id: RequestId::Number(1),
            method: std::sync::Arc::from("fs/write_text_file"),
            params: Some(WriteTextFileRequest::new(
                agent_client_protocol::SessionId::from("sess-1"),
                "/tmp/file.txt".to_string(),
                "content".to_string(),
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
            method: std::sync::Arc::from("fs/write_text_file"),
            params: Some(WriteTextFileRequest::new(
                agent_client_protocol::SessionId::from("sess-1"),
                "/forbidden.txt".to_string(),
                "content".to_string(),
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
        let client = MockClient;
        let envelope = Request::<WriteTextFileRequest> {
            id: RequestId::Number(1),
            method: std::sync::Arc::from("fs/write_text_file"),
            params: None,
        };
        let payload = serde_json::to_vec(&envelope).unwrap();

        let result = forward_to_client(&payload, &client, "sess-1").await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), FsWriteTextFileError::InvalidRequest(_)));
    }

    #[tokio::test]
    async fn forward_to_client_session_id_mismatch_returns_invalid_request() {
        let client = MockClient;
        let envelope = Request {
            id: RequestId::Number(1),
            method: std::sync::Arc::from("fs/write_text_file"),
            params: Some(WriteTextFileRequest::new(
                agent_client_protocol::SessionId::from("sess-1"),
                "/tmp/foo.txt".to_string(),
                "content".to_string(),
            )),
        };
        let payload = serde_json::to_vec(&envelope).unwrap();

        let result = forward_to_client(&payload, &client, "sess-other").await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), FsWriteTextFileError::InvalidRequest(_)));
    }

    #[test]
    fn fs_write_text_file_error_display() {
        let err = serde_json::from_slice::<WriteTextFileRequest>(b"not json").unwrap_err();
        let fs_err = FsWriteTextFileError::InvalidRequest(err);
        assert!(fs_err.to_string().contains("invalid request"));

        let client_err = agent_client_protocol::Error::new(ErrorCode::InvalidParams.into(), "permission denied");
        let fs_err = FsWriteTextFileError::ClientError(client_err);
        assert!(fs_err.to_string().contains("client error"));
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
}
