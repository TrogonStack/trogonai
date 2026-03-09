use crate::client::rpc_reply;
use crate::jsonrpc::extract_request_id;
use crate::nats::{FlushClient, PublishClient};
use agent_client_protocol::{
    Client, ErrorCode, Request, Response, TerminalOutputRequest, TerminalOutputResponse,
};
use bytes::Bytes;
use tracing::{instrument, warn};
use trogon_std::JsonSerialize;

#[derive(Debug)]
pub enum TerminalOutputError {
    MalformedJson(serde_json::Error),
    InvalidParams(agent_client_protocol::Error),
    ClientError(agent_client_protocol::Error),
}

impl std::fmt::Display for TerminalOutputError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MalformedJson(e) => write!(f, "malformed JSON: {}", e),
            Self::InvalidParams(e) => write!(f, "invalid params: {}", e),
            Self::ClientError(e) => write!(f, "client error: {}", e),
        }
    }
}

impl std::error::Error for TerminalOutputError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::MalformedJson(e) => Some(e),
            Self::InvalidParams(e) => Some(e),
            Self::ClientError(e) => Some(e),
        }
    }
}

fn invalid_params_error(message: impl Into<String>) -> TerminalOutputError {
    TerminalOutputError::InvalidParams(agent_client_protocol::Error::new(
        i32::from(ErrorCode::InvalidParams),
        message.into(),
    ))
}

pub fn error_code_and_message(e: &TerminalOutputError) -> (ErrorCode, String) {
    match e {
        TerminalOutputError::MalformedJson(inner) => (
            ErrorCode::ParseError,
            format!("Malformed terminal/output request JSON: {}", inner),
        ),
        TerminalOutputError::InvalidParams(inner) => (inner.code, inner.message.clone()),
        TerminalOutputError::ClientError(inner) => (inner.code, inner.message.clone()),
    }
}

/// Handles terminal/output: parses request, calls client, wraps response in JSON-RPC envelope,
/// and publishes to reply subject. Reply is required (request-reply pattern).
#[instrument(
    name = "acp.client.terminal.output",
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
                "terminal/output requires reply subject; ignoring message"
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
                "terminal_output reply",
            )
            .await;
        }
        Err(e) => {
            let (code, message) = error_code_and_message(&e);
            warn!(
                error = %e,
                session_id = %session_id,
                "Failed to handle terminal/output"
            );
            let (bytes, content_type) =
                rpc_reply::error_response_bytes(serializer, request_id, code, &message);
            rpc_reply::publish_reply(
                nats,
                reply_to,
                bytes,
                content_type,
                "terminal_output error reply",
            )
            .await;
        }
    }
}

async fn forward_to_client<C: Client>(
    payload: &[u8],
    client: &C,
    expected_session_id: &str,
) -> Result<TerminalOutputResponse, TerminalOutputError> {
    let payload_value: serde_json::Value =
        serde_json::from_slice(payload).map_err(TerminalOutputError::MalformedJson)?;
    let envelope: Request<TerminalOutputRequest> = serde_json::from_value(payload_value)
        .map_err(|e| invalid_params_error(format!("Invalid terminal/output request: {}", e)))?;
    let request = envelope
        .params
        .ok_or_else(|| invalid_params_error("params is null or missing"))?;
    let params_session_id = request.session_id.to_string();
    if params_session_id != expected_session_id {
        return Err(invalid_params_error(format!(
            "params.sessionId ({}) does not match subject session id ({})",
            params_session_id, expected_session_id
        )));
    }
    client
        .terminal_output(request)
        .await
        .map_err(TerminalOutputError::ClientError)
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::{
        ContentBlock, ContentChunk, Request, RequestId, RequestPermissionRequest,
        RequestPermissionResponse, SessionNotification, SessionUpdate, TerminalOutputRequest,
        TerminalOutputResponse,
    };
    use async_trait::async_trait;
    use std::error::Error;
    use trogon_nats::{AdvancedMockNatsClient, MockNatsClient};
    use trogon_std::{FailNextSerialize, StdJsonSerialize};

    struct MockClient;

    impl MockClient {
        fn new() -> Self {
            Self
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

        async fn terminal_output(
            &self,
            _: TerminalOutputRequest,
        ) -> agent_client_protocol::Result<TerminalOutputResponse> {
            Ok(TerminalOutputResponse::new(
                "output data".to_string(),
                false,
            ))
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

        async fn terminal_output(
            &self,
            _: TerminalOutputRequest,
        ) -> agent_client_protocol::Result<TerminalOutputResponse> {
            Err(agent_client_protocol::Error::new(
                -32603,
                "mock terminal_output failure",
            ))
        }
    }

    #[tokio::test]
    async fn handle_success_publishes_response_to_reply_subject() {
        let nats = MockNatsClient::new();
        let client = MockClient::new();
        let request = TerminalOutputRequest::new("sess-1", "term-001");
        let envelope = Request {
            id: RequestId::Number(1),
            method: std::sync::Arc::from("terminal/output"),
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
        let client = MockClient::new();
        let request = TerminalOutputRequest::new("sess-1", "term-001");
        let envelope = Request {
            id: RequestId::Number(1),
            method: std::sync::Arc::from("terminal/output"),
            params: Some(request),
        };
        let payload = serde_json::to_vec(&envelope).unwrap();

        handle(&payload, &client, None, &nats, "sess-1", &StdJsonSerialize).await;

        assert!(nats.published_messages().is_empty());
    }

    #[tokio::test]
    async fn handle_invalid_payload_publishes_error_reply() {
        let nats = MockNatsClient::new();
        let client = MockClient::new();

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
        let response: serde_json::Value = serde_json::from_slice(payloads[0].as_ref()).unwrap();
        assert_eq!(
            response.get("error").and_then(|e| e.get("code")),
            Some(&serde_json::Value::from(-32700)),
            "malformed JSON should return ParseError (-32700)"
        );
    }

    #[tokio::test]
    async fn handle_invalid_params_publishes_invalid_params_error() {
        let nats = MockNatsClient::new();
        let client = MockClient::new();
        let payload = br#"{"id":1,"method":"terminal/output","params":{}}"#;

        handle(
            payload.as_slice(),
            &client,
            Some("_INBOX.err"),
            &nats,
            "sess-1",
            &StdJsonSerialize,
        )
        .await;

        assert_eq!(nats.published_messages(), vec!["_INBOX.err"]);
        let payloads = nats.published_payloads();
        let response: serde_json::Value = serde_json::from_slice(payloads[0].as_ref()).unwrap();
        assert_eq!(
            response.get("error").and_then(|e| e.get("code")),
            Some(&serde_json::Value::from(-32602)),
            "valid JSON with invalid params should return InvalidParams (-32602)"
        );
    }

    #[tokio::test]
    async fn handle_client_error_publishes_error_reply() {
        let nats = MockNatsClient::new();
        let client = FailingClient;
        let request = TerminalOutputRequest::new("sess-1", "term-001");
        let envelope = Request {
            id: RequestId::Number(1),
            method: std::sync::Arc::from("terminal/output"),
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

    #[tokio::test]
    async fn handle_session_id_mismatch_publishes_error_reply() {
        let nats = MockNatsClient::new();
        let client = MockClient::new();
        let request = TerminalOutputRequest::new("sess-b", "term-001");
        let envelope = Request {
            id: RequestId::Number(1),
            method: std::sync::Arc::from("terminal/output"),
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
        let client = MockClient::new();
        let serializer = FailNextSerialize::new(1);
        let request = TerminalOutputRequest::new("sess-1", "term-001");
        let envelope = Request {
            id: RequestId::Number(1),
            method: std::sync::Arc::from("terminal/output"),
            params: Some(request),
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
    async fn handle_success_publish_failure_exercises_error_path() {
        let nats = AdvancedMockNatsClient::new();
        nats.fail_next_publish();
        let client = MockClient::new();
        let request = TerminalOutputRequest::new("sess-1", "term-001");
        let envelope = Request {
            id: RequestId::Number(1),
            method: std::sync::Arc::from("terminal/output"),
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

        assert!(nats.published_messages().is_empty());
    }

    #[tokio::test]
    async fn handle_success_flush_failure_exercises_warn_path() {
        let nats = AdvancedMockNatsClient::new();
        nats.fail_next_flush();
        let client = MockClient::new();
        let request = TerminalOutputRequest::new("sess-1", "term-001");
        let envelope = Request {
            id: RequestId::Number(1),
            method: std::sync::Arc::from("terminal/output"),
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

    #[test]
    fn error_code_and_message_malformed_json_returns_parse_error() {
        let err = serde_json::from_slice::<serde_json::Value>(b"not json").unwrap_err();
        let to_err = TerminalOutputError::MalformedJson(err);
        let (code, message) = error_code_and_message(&to_err);
        assert_eq!(code, ErrorCode::ParseError);
        assert!(message.contains("Malformed terminal/output request JSON"));
    }

    #[test]
    fn error_code_and_message_invalid_params_preserves_code_and_message() {
        let inner =
            agent_client_protocol::Error::new(ErrorCode::InvalidParams.into(), "params is null");
        let to_err = TerminalOutputError::InvalidParams(inner);
        let (code, message) = error_code_and_message(&to_err);
        assert_eq!(code, ErrorCode::InvalidParams);
        assert_eq!(message, "params is null");
    }

    #[test]
    fn error_code_and_message_client_error_preserves_client_code() {
        let client_err =
            agent_client_protocol::Error::new(ErrorCode::InvalidParams.into(), "denied");
        let to_err = TerminalOutputError::ClientError(client_err);
        let (code, message) = error_code_and_message(&to_err);
        assert_eq!(code, ErrorCode::InvalidParams);
        assert_eq!(message, "denied");
    }

    #[test]
    fn terminal_output_error_display() {
        let malformed = TerminalOutputError::MalformedJson(
            serde_json::from_slice::<serde_json::Value>(b"not json").unwrap_err(),
        );
        assert!(malformed.to_string().contains("malformed JSON"));

        let invalid_params = TerminalOutputError::InvalidParams(agent_client_protocol::Error::new(
            ErrorCode::InvalidParams.into(),
            "bad params",
        ));
        assert!(invalid_params.to_string().contains("invalid params"));

        let client_err = TerminalOutputError::ClientError(agent_client_protocol::Error::new(
            ErrorCode::InvalidParams.into(),
            "client fail",
        ));
        assert!(client_err.to_string().contains("client error"));
    }

    #[test]
    fn terminal_output_error_source() {
        let malformed = TerminalOutputError::MalformedJson(
            serde_json::from_slice::<serde_json::Value>(b"not json").unwrap_err(),
        );
        assert!(malformed.source().is_some());

        let invalid_params = TerminalOutputError::InvalidParams(agent_client_protocol::Error::new(
            ErrorCode::InvalidParams.into(),
            "bad params",
        ));
        assert!(invalid_params.source().is_some());

        let client_err = TerminalOutputError::ClientError(agent_client_protocol::Error::new(
            ErrorCode::InvalidParams.into(),
            "client fail",
        ));
        assert!(client_err.source().is_some());
    }

    #[tokio::test]
    async fn mock_client_session_notification_returns_ok() {
        let client = MockClient::new();
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
        let client = MockClient::new();
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
