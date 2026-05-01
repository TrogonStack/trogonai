use crate::client::rpc_reply;
use crate::nats::{FlushClient, PublishClient};
use agent_client_protocol::{
    Client, ErrorCode, RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse,
};
use bytes::Bytes;
use serde::de::Error as SerdeDeError;
use tracing::{instrument, warn};
use trogon_std::JsonSerialize;

#[derive(Debug)]
pub enum RequestPermissionError {
    InvalidRequest(serde_json::Error),
    ClientError(agent_client_protocol::Error),
}

impl std::fmt::Display for RequestPermissionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidRequest(e) => write!(f, "invalid request: {}", e),
            Self::ClientError(e) => write!(f, "client error: {}", e),
        }
    }
}

impl std::error::Error for RequestPermissionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidRequest(e) => Some(e),
            Self::ClientError(e) => Some(e),
        }
    }
}

pub fn error_code_and_message(e: &RequestPermissionError) -> (ErrorCode, String) {
    match e {
        RequestPermissionError::InvalidRequest(inner) => (
            ErrorCode::InvalidParams,
            format!("Invalid request_permission request: {}", inner),
        ),
        RequestPermissionError::ClientError(inner) => (inner.code, inner.message.clone()),
    }
}

/// Handles session/request_permission: parses raw JSON request, calls client, publishes raw JSON
/// response to reply subject. Reply is required (request-reply pattern).
///
/// The payload is raw JSON (`RequestPermissionRequest`), not JSON-RPC-wrapped, matching the format
/// that `NatsClientProxy::request_permission` sends.  On error the handler publishes a
/// `RequestPermissionResponse` with `Cancelled` outcome so the caller receives a valid (deny)
/// decision instead of a deserialization failure.
#[instrument(
    name = "acp.client.session.request_permission",
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
                "request_permission requires reply subject; ignoring message"
            );
            return;
        }
    };

    let cancelled = RequestPermissionResponse::new(RequestPermissionOutcome::Cancelled);

    match forward_to_client(payload, client, session_id).await {
        Ok(response) => {
            let (response_bytes, content_type) = serializer
                .to_vec(&response)
                .map(|v| (Bytes::from(v), rpc_reply::CONTENT_TYPE_JSON))
                .unwrap_or_else(|e| {
                    warn!(error = %e, "JSON serialization of response failed, sending cancelled reply");
                    serializer
                        .to_vec(&cancelled)
                        .map(|v| (Bytes::from(v), rpc_reply::CONTENT_TYPE_JSON))
                        .unwrap_or_else(|_| (Bytes::new(), rpc_reply::CONTENT_TYPE_JSON))
                });
            rpc_reply::publish_reply(nats, reply_to, response_bytes, content_type, "request_permission reply").await;
        }
        Err(e) => {
            let (_, message) = error_code_and_message(&e);
            warn!(
                error = %message,
                session_id = %session_id,
                "Failed to handle request_permission — replying Cancelled"
            );
            let (bytes, content_type) = serializer
                .to_vec(&cancelled)
                .map(|v| (Bytes::from(v), rpc_reply::CONTENT_TYPE_JSON))
                .unwrap_or_else(|_| (Bytes::new(), rpc_reply::CONTENT_TYPE_JSON));
            rpc_reply::publish_reply(nats, reply_to, bytes, content_type, "request_permission error reply").await;
        }
    }
}

async fn forward_to_client<C: Client>(
    payload: &[u8],
    client: &C,
    expected_session_id: &str,
) -> Result<RequestPermissionResponse, RequestPermissionError> {
    let request: RequestPermissionRequest =
        serde_json::from_slice(payload).map_err(RequestPermissionError::InvalidRequest)?;
    let params_session_id = request.session_id.to_string();
    if params_session_id != expected_session_id {
        return Err(RequestPermissionError::InvalidRequest(serde_json::Error::custom(format!(
            "params.sessionId ({}) does not match subject session id ({})",
            params_session_id, expected_session_id
        ))));
    }
    client
        .request_permission(request)
        .await
        .map_err(RequestPermissionError::ClientError)
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::{
        ContentBlock, ContentChunk, PermissionOption, PermissionOptionKind, RequestPermissionOutcome,
        RequestPermissionResponse, SessionNotification, SessionUpdate, ToolCallUpdate, ToolCallUpdateFields,
    };
    use std::error::Error;
    use trogon_nats::{AdvancedMockNatsClient, MockNatsClient};
    use trogon_std::{FailNextSerialize, StdJsonSerialize};

    struct MockClient {
        outcome: RequestPermissionOutcome,
    }

    impl MockClient {
        fn new(outcome: RequestPermissionOutcome) -> Self {
            Self { outcome }
        }
    }

    #[async_trait::async_trait(?Send)]
    impl agent_client_protocol::Client for MockClient {
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

    #[async_trait::async_trait(?Send)]
    impl agent_client_protocol::Client for FailingClient {
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

    fn make_raw_payload(request: RequestPermissionRequest) -> Vec<u8> {
        serde_json::to_vec(&request).unwrap()
    }

    fn make_request() -> RequestPermissionRequest {
        let tool_call = ToolCallUpdate::new("call-1", ToolCallUpdateFields::new());
        let options = vec![PermissionOption::new(
            "allow-once",
            "Allow once",
            PermissionOptionKind::AllowOnce,
        )];
        RequestPermissionRequest::new("session-001", tool_call, options)
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
        let request = make_request();
        let payload = make_raw_payload(request);

        let result = forward_to_client(&payload, &client, "session-001").await;
        assert!(result.is_ok());
        let response = result.unwrap();
        assert_eq!(response.outcome, RequestPermissionOutcome::Cancelled);
    }

    #[tokio::test]
    async fn request_permission_returns_error_when_payload_is_invalid_json() {
        let client = MockClient::new(RequestPermissionOutcome::Cancelled);
        let result = forward_to_client(b"not json", &client, "session-001").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn request_permission_returns_client_error_when_client_fails() {
        let client = FailingClient;
        let payload = make_raw_payload(make_request());

        let result = forward_to_client(&payload, &client, "session-001").await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), RequestPermissionError::ClientError(_)));
    }

    #[tokio::test]
    async fn request_permission_returns_invalid_request_when_session_id_mismatch() {
        let client = MockClient::new(RequestPermissionOutcome::Cancelled);
        let tool_call = ToolCallUpdate::new("call-1", ToolCallUpdateFields::new());
        let request = RequestPermissionRequest::new("session-other", tool_call, vec![]);
        let payload = make_raw_payload(request);

        let result = forward_to_client(&payload, &client, "session-001").await;
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
        let rp_err = RequestPermissionError::InvalidRequest(err);
        assert!(rp_err.to_string().contains("invalid request"));

        let client_err = agent_client_protocol::Error::new(ErrorCode::InvalidParams.into(), "permission denied");
        let rp_err = RequestPermissionError::ClientError(client_err);
        assert!(rp_err.to_string().contains("client error"));
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
        let payload = make_raw_payload(make_request());

        handle(&payload, &client, Some("_INBOX.reply"), &nats, "session-001", &StdJsonSerialize).await;

        assert_eq!(nats.published_messages(), vec!["_INBOX.reply"]);
        // Verify the reply is a valid RequestPermissionResponse (raw JSON, not JSON-RPC wrapped)
        let payloads = nats.published_payloads();
        let parsed: RequestPermissionResponse = serde_json::from_slice(payloads[0].as_ref()).unwrap();
        assert_eq!(parsed.outcome, RequestPermissionOutcome::Cancelled);
    }

    #[tokio::test]
    async fn handle_no_reply_does_not_publish() {
        let nats = MockNatsClient::new();
        let client = MockClient::new(RequestPermissionOutcome::Cancelled);
        let payload = make_raw_payload(make_request());

        handle(&payload, &client, None, &nats, "session-001", &StdJsonSerialize).await;

        assert!(nats.published_messages().is_empty());
    }

    #[tokio::test]
    async fn handle_session_id_mismatch_publishes_cancelled_reply() {
        let nats = MockNatsClient::new();
        let client = MockClient::new(RequestPermissionOutcome::Cancelled);
        let tool_call = ToolCallUpdate::new("call-1", ToolCallUpdateFields::new());
        let request = RequestPermissionRequest::new("session-other", tool_call, vec![]);
        let payload = make_raw_payload(request);

        handle(&payload, &client, Some("_INBOX.err"), &nats, "session-001", &StdJsonSerialize).await;

        assert_eq!(nats.published_messages(), vec!["_INBOX.err"]);
        // Error path publishes a Cancelled response
        let payloads = nats.published_payloads();
        let parsed: RequestPermissionResponse = serde_json::from_slice(payloads[0].as_ref()).unwrap();
        assert_eq!(parsed.outcome, RequestPermissionOutcome::Cancelled);
    }

    #[tokio::test]
    async fn handle_invalid_payload_publishes_cancelled_reply() {
        let nats = MockNatsClient::new();
        let client = MockClient::new(RequestPermissionOutcome::Cancelled);

        handle(b"not json", &client, Some("_INBOX.err"), &nats, "session-001", &StdJsonSerialize).await;

        assert_eq!(nats.published_messages(), vec!["_INBOX.err"]);
        let payloads = nats.published_payloads();
        let parsed: RequestPermissionResponse = serde_json::from_slice(payloads[0].as_ref()).unwrap();
        assert_eq!(parsed.outcome, RequestPermissionOutcome::Cancelled);
    }

    #[tokio::test]
    async fn handle_client_error_publishes_cancelled_reply() {
        let nats = MockNatsClient::new();
        let client = FailingClient;
        let payload = make_raw_payload(make_request());

        handle(&payload, &client, Some("_INBOX.err"), &nats, "session-001", &StdJsonSerialize).await;

        assert_eq!(nats.published_messages(), vec!["_INBOX.err"]);
        let payloads = nats.published_payloads();
        let parsed: RequestPermissionResponse = serde_json::from_slice(payloads[0].as_ref()).unwrap();
        assert_eq!(parsed.outcome, RequestPermissionOutcome::Cancelled);
    }

    #[tokio::test]
    async fn handle_success_serialization_fallback_sends_cancelled_reply() {
        let nats = MockNatsClient::new();
        let client = MockClient::new(RequestPermissionOutcome::Cancelled);
        let serializer = FailNextSerialize::new(1);
        let payload = make_raw_payload(make_request());

        handle(&payload, &client, Some("_INBOX.reply"), &nats, "session-001", &serializer).await;

        assert_eq!(nats.published_messages(), vec!["_INBOX.reply"]);
    }

    #[tokio::test]
    async fn handle_success_flush_failure_exercises_warn_path() {
        let nats = AdvancedMockNatsClient::new();
        nats.fail_next_flush();
        let client = MockClient::new(RequestPermissionOutcome::Cancelled);
        let payload = make_raw_payload(make_request());

        handle(&payload, &client, Some("_INBOX.reply"), &nats, "session-001", &StdJsonSerialize).await;

        assert_eq!(nats.published_messages(), vec!["_INBOX.reply"]);
    }

    #[tokio::test]
    async fn handle_success_publish_failure_exercises_error_path() {
        let nats = AdvancedMockNatsClient::new();
        nats.fail_next_publish();
        let client = MockClient::new(RequestPermissionOutcome::Cancelled);
        let payload = make_raw_payload(make_request());

        handle(&payload, &client, Some("_INBOX.reply"), &nats, "session-001", &StdJsonSerialize).await;

        assert!(nats.published_messages().is_empty());
    }

    #[tokio::test]
    async fn handle_client_error_serialization_last_resort_exercises_empty_path() {
        let nats = MockNatsClient::new();
        let client = FailingClient;
        let serializer = FailNextSerialize::new(2);
        let payload = make_raw_payload(make_request());

        handle(&payload, &client, Some("_INBOX.err"), &nats, "session-001", &serializer).await;

        // Both cancelled serialization attempts fail → empty bytes published
        assert_eq!(nats.published_messages(), vec!["_INBOX.err"]);
    }
}
