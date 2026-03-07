use crate::client::rpc_reply;
use crate::jsonrpc::extract_request_id;
use crate::nats::{FlushClient, PublishClient};
use agent_client_protocol::{
    Client, CreateTerminalRequest, CreateTerminalResponse, ErrorCode, Request, Response,
};
use bytes::Bytes;
use serde::de::Error as SerdeDeError;
use tracing::{instrument, warn};
use trogon_std::JsonSerialize;

#[derive(Debug)]
pub enum TerminalCreateError {
    InvalidRequest(serde_json::Error),
    ClientError(agent_client_protocol::Error),
}

impl std::fmt::Display for TerminalCreateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidRequest(e) => write!(f, "invalid request: {}", e),
            Self::ClientError(e) => write!(f, "client error: {}", e),
        }
    }
}

impl std::error::Error for TerminalCreateError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidRequest(e) => Some(e),
            Self::ClientError(e) => Some(e),
        }
    }
}

pub fn error_code_and_message(e: &TerminalCreateError) -> (ErrorCode, String) {
    match e {
        TerminalCreateError::InvalidRequest(inner) => (
            ErrorCode::InvalidParams,
            format!("Invalid terminal/create request: {}", inner),
        ),
        TerminalCreateError::ClientError(inner) => (inner.code, inner.message.clone()),
    }
}

/// Handles terminal/create: parses request, calls client, wraps response in JSON-RPC envelope,
/// and publishes to reply subject. Reply is required (request-reply pattern).
#[instrument(
    name = "acp.client.terminal.create",
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
                "terminal/create requires reply subject; ignoring message"
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
            rpc_reply::publish_reply(
                nats,
                reply_to,
                response_bytes,
                content_type,
                "terminal_create reply",
            )
            .await;
        }
        Err(e) => {
            let (code, message) = error_code_and_message(&e);
            warn!(
                error = %e,
                session_id = %session_id,
                "Failed to handle terminal/create"
            );
            let (bytes, content_type) =
                rpc_reply::error_response_bytes(serializer, request_id, code, &message);
            rpc_reply::publish_reply(
                nats,
                reply_to,
                bytes,
                content_type,
                "terminal_create error reply",
            )
            .await;
        }
    }
}

async fn forward_to_client<C: Client>(
    payload: &[u8],
    client: &C,
    expected_session_id: &str,
) -> Result<CreateTerminalResponse, TerminalCreateError> {
    let envelope: Request<CreateTerminalRequest> =
        serde_json::from_slice(payload).map_err(TerminalCreateError::InvalidRequest)?;
    let request = envelope.params.ok_or_else(|| {
        TerminalCreateError::InvalidRequest(serde_json::Error::custom("params is null or missing"))
    })?;
    let params_session_id = request.session_id.to_string();
    if params_session_id != expected_session_id {
        return Err(TerminalCreateError::InvalidRequest(
            serde_json::Error::custom(format!(
                "params.sessionId ({}) does not match subject session id ({})",
                params_session_id, expected_session_id
            )),
        ));
    }
    client
        .create_terminal(request)
        .await
        .map_err(TerminalCreateError::ClientError)
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::{
        ContentBlock, ContentChunk, CreateTerminalRequest, CreateTerminalResponse, Request,
        RequestId, RequestPermissionRequest, RequestPermissionResponse, SessionNotification,
        SessionUpdate,
    };
    use async_trait::async_trait;
    use std::error::Error;
    use trogon_nats::MockNatsClient;
    use trogon_std::{FailNextSerialize, StdJsonSerialize};

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

        async fn create_terminal(
            &self,
            _: CreateTerminalRequest,
        ) -> agent_client_protocol::Result<CreateTerminalResponse> {
            Ok(CreateTerminalResponse::new(self.terminal_id.clone()))
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

        async fn create_terminal(
            &self,
            _: CreateTerminalRequest,
        ) -> agent_client_protocol::Result<CreateTerminalResponse> {
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
        let envelope = Request {
            id: RequestId::Number(1),
            method: std::sync::Arc::from("terminal/create"),
            params: Some(request),
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
        let client = MockClient::new("term-001");
        let request = CreateTerminalRequest::new("sess-1", "echo hello");
        let envelope = Request {
            id: RequestId::Number(1),
            method: std::sync::Arc::from("terminal/create"),
            params: Some(request),
        };
        let payload = serde_json::to_vec(&envelope).unwrap();

        handle(&payload, &client, None, &nats, "sess-1", &StdJsonSerialize).await;

        assert!(nats.published_messages().is_empty());
    }

    #[tokio::test]
    async fn handle_invalid_payload_publishes_error_reply() {
        let nats = MockNatsClient::new();
        let client = MockClient::new("term-001");

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
    async fn handle_client_error_publishes_error_reply() {
        let nats = MockNatsClient::new();
        let client = FailingClient;
        let request = CreateTerminalRequest::new("sess-1", "echo hello");
        let envelope = Request {
            id: RequestId::Number(1),
            method: std::sync::Arc::from("terminal/create"),
            params: Some(request),
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
        let err = serde_json::from_slice::<CreateTerminalRequest>(b"not json").unwrap_err();
        let tc_err = TerminalCreateError::InvalidRequest(err);
        let (code, message) = error_code_and_message(&tc_err);
        assert_eq!(code, ErrorCode::InvalidParams);
        assert!(message.contains("Invalid terminal/create request"));
    }

    #[test]
    fn error_code_and_message_client_error_preserves_client_code() {
        let client_err =
            agent_client_protocol::Error::new(ErrorCode::InvalidParams.into(), "file not found");
        let tc_err = TerminalCreateError::ClientError(client_err);
        let (code, message) = error_code_and_message(&tc_err);
        assert_eq!(code, ErrorCode::InvalidParams);
        assert_eq!(message, "file not found");
    }

    #[test]
    fn error_code_and_message_client_error_preserves_other_code() {
        let client_err =
            agent_client_protocol::Error::new(ErrorCode::InternalError.into(), "internal");
        let tc_err = TerminalCreateError::ClientError(client_err);
        let (code, _) = error_code_and_message(&tc_err);
        assert_eq!(code, ErrorCode::InternalError);
    }

    #[tokio::test]
    async fn forward_to_client_params_none_returns_invalid_request() {
        let client = MockClient::new("term-001");
        let envelope = Request::<CreateTerminalRequest> {
            id: RequestId::Number(1),
            method: std::sync::Arc::from("terminal/create"),
            params: None,
        };
        let payload = serde_json::to_vec(&envelope).unwrap();

        let result = forward_to_client(&payload, &client, "sess-1").await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            TerminalCreateError::InvalidRequest(_)
        ));
    }

    #[tokio::test]
    async fn forward_to_client_session_id_mismatch_returns_invalid_request() {
        let client = MockClient::new("term-001");
        let request = CreateTerminalRequest::new("sess-b", "echo hello");
        let envelope = Request {
            id: RequestId::Number(1),
            method: std::sync::Arc::from("terminal/create"),
            params: Some(request),
        };
        let payload = serde_json::to_vec(&envelope).unwrap();

        let result = forward_to_client(&payload, &client, "sess-a").await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            TerminalCreateError::InvalidRequest(_)
        ));
    }

    #[tokio::test]
    async fn handle_session_id_mismatch_publishes_error_reply() {
        let nats = MockNatsClient::new();
        let client = MockClient::new("term-001");
        let request = CreateTerminalRequest::new("sess-b", "echo hello");
        let envelope = Request {
            id: RequestId::Number(1),
            method: std::sync::Arc::from("terminal/create"),
            params: Some(request),
        };
        let payload = serde_json::to_vec(&envelope).unwrap();

        handle(
            &payload,
            &client,
            Some("_INBOX.err"),
            &nats,
            "sess-a",
            &StdJsonSerialize,
        )
        .await;

        assert_eq!(nats.published_messages(), vec!["_INBOX.err"]);
    }

    #[tokio::test]
    async fn handle_success_serialization_fallback_sends_error_reply() {
        let nats = MockNatsClient::new();
        let client = MockClient::new("term-001");
        let serializer = FailNextSerialize::new(1);
        let envelope = Request {
            id: RequestId::Number(1),
            method: std::sync::Arc::from("terminal/create"),
            params: Some(CreateTerminalRequest::new("sess-1", "echo hello")),
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

    #[test]
    fn terminal_create_error_display() {
        let err = serde_json::from_slice::<CreateTerminalRequest>(b"not json").unwrap_err();
        let tc_err = TerminalCreateError::InvalidRequest(err);
        assert!(tc_err.to_string().contains("invalid request"));

        let client_err =
            agent_client_protocol::Error::new(ErrorCode::InvalidParams.into(), "create failed");
        let tc_err = TerminalCreateError::ClientError(client_err);
        assert!(tc_err.to_string().contains("client error"));
    }

    #[test]
    fn terminal_create_error_source() {
        let err = serde_json::from_slice::<CreateTerminalRequest>(b"not json").unwrap_err();
        let tc_err = TerminalCreateError::InvalidRequest(err);
        assert!(tc_err.source().is_some());

        let client_err =
            agent_client_protocol::Error::new(ErrorCode::InvalidParams.into(), "create failed");
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
}
