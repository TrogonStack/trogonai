use super::*;
use agent_client_protocol::schema::v1::{
    ContentBlock, ContentChunk, ElicitationAcceptAction, ElicitationFormMode, ElicitationRequestScope,
    ElicitationSchema, ElicitationSessionScope, RequestPermissionOutcome, RequestPermissionRequest,
    RequestPermissionResponse, SessionNotification, SessionUpdate,
};
use async_nats::header::HeaderMap;
use jsonrpc_nats::RequestId;
use std::error::Error;
use trogon_nats::{AdvancedMockNatsClient, MockNatsClient};

struct MockClient {
    action: ElicitationAcceptAction,
}

impl MockClient {
    fn new(action: ElicitationAcceptAction) -> Self {
        Self { action }
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
        Ok(RequestPermissionResponse::new(RequestPermissionOutcome::Cancelled))
    }

    async fn elicitation_create(
        &self,
        _: CreateElicitationRequest,
    ) -> agent_client_protocol::Result<CreateElicitationResponse> {
        Ok(CreateElicitationResponse::new(self.action.clone()))
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
        Ok(RequestPermissionResponse::new(RequestPermissionOutcome::Cancelled))
    }

    async fn elicitation_create(
        &self,
        _: CreateElicitationRequest,
    ) -> agent_client_protocol::Result<CreateElicitationResponse> {
        Err(agent_client_protocol::Error::new(
            i32::from(ErrorCode::InvalidParams),
            "elicitation declined",
        ))
    }
}

fn session_scoped_request(session_id: &'static str) -> CreateElicitationRequest {
    let mode = ElicitationFormMode::new(ElicitationSessionScope::new(session_id), ElicitationSchema::new());
    CreateElicitationRequest::new(mode, "Please provide input")
}

fn request_scoped_request() -> CreateElicitationRequest {
    let mode = ElicitationFormMode::new(
        ElicitationRequestScope::new(agent_client_protocol::schema::v1::RequestId::from(7i64)),
        ElicitationSchema::new(),
    );
    CreateElicitationRequest::new(mode, "Please authenticate")
}

fn make_wire_request(request: CreateElicitationRequest) -> (HeaderMap, Vec<u8>) {
    crate::client::test_support::encode_wire_request("elicitation/create", RequestId::Number(1), &request)
}

fn empty_headers() -> HeaderMap {
    HeaderMap::new()
}

#[tokio::test]
async fn mock_client_session_notification_returns_ok() {
    let client = MockClient::new(ElicitationAcceptAction::new());
    let notification = SessionNotification::new(
        "sess-1",
        SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::from("hi"))),
    );
    let result = client.session_notification(notification).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn elicitation_create_forwards_session_scoped_request_and_returns_response() {
    let client = MockClient::new(ElicitationAcceptAction::new());
    let request = session_scoped_request("session-001");
    let (headers, payload) = make_wire_request(request);

    let result = forward_to_client(&headers, &payload, &client, "session-001").await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn elicitation_create_forwards_request_scoped_request_without_session_check() {
    let client = MockClient::new(ElicitationAcceptAction::new());
    let request = request_scoped_request();
    let (headers, payload) = make_wire_request(request);

    let result = forward_to_client(&headers, &payload, &client, "session-001").await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn elicitation_create_returns_error_when_payload_is_invalid_json() {
    let client = MockClient::new(ElicitationAcceptAction::new());
    let result = forward_to_client(&empty_headers(), b"not json", &client, "session-001").await;
    assert!(result.is_err());
}

#[tokio::test]
async fn elicitation_create_returns_client_error_when_client_fails() {
    let client = FailingClient;
    let request = session_scoped_request("session-001");
    let (headers, payload) = make_wire_request(request);

    let result = forward_to_client(&headers, &payload, &client, "session-001").await;
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), ElicitationCreateError::ClientError(_)));
}

#[tokio::test]
async fn elicitation_create_returns_invalid_request_when_params_missing() {
    let client = MockClient::new(ElicitationAcceptAction::new());
    let result = forward_to_client(&empty_headers(), b"{}", &client, "session-001").await;
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), ElicitationCreateError::InvalidRequest(_)));
}

#[tokio::test]
async fn elicitation_create_returns_invalid_request_when_session_id_mismatch() {
    let client = MockClient::new(ElicitationAcceptAction::new());
    let request = session_scoped_request("session-other");
    let (headers, payload) = make_wire_request(request);

    let result = forward_to_client(&headers, &payload, &client, "session-001").await;
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), ElicitationCreateError::InvalidRequest(_)));
}

#[test]
fn error_code_and_message_invalid_request_returns_invalid_params() {
    let err = serde_json::from_slice::<CreateElicitationRequest>(b"not json").unwrap_err();
    let ec_err = ElicitationCreateError::InvalidRequest(err);
    let (code, _) = error_code_and_message(&ec_err);
    assert_eq!(code, ErrorCode::InvalidParams);
}

#[test]
fn error_code_and_message_client_error_preserves_client_code() {
    let client_err = agent_client_protocol::Error::new(ErrorCode::InvalidParams.into(), "denied");
    let ec_err = ElicitationCreateError::ClientError(client_err);
    let (code, message) = error_code_and_message(&ec_err);
    assert_eq!(code, ErrorCode::InvalidParams);
    assert_eq!(message, "denied");
}

#[test]
fn elicitation_create_error_display() {
    let err = serde_json::from_slice::<CreateElicitationRequest>(b"not json").unwrap_err();
    let expected = format!("invalid request: {err}");
    let ec_err = ElicitationCreateError::InvalidRequest(err);
    assert_eq!(ec_err.to_string(), expected);

    let client_err = agent_client_protocol::Error::new(ErrorCode::InvalidParams.into(), "elicitation declined");
    let ec_err = ElicitationCreateError::ClientError(client_err);
    assert_eq!(ec_err.to_string(), "client error: elicitation declined");
}

#[test]
fn elicitation_create_error_source() {
    let err = serde_json::from_slice::<CreateElicitationRequest>(b"not json").unwrap_err();
    let ec_err = ElicitationCreateError::InvalidRequest(err);
    assert!(ec_err.source().is_some());

    let client_err = agent_client_protocol::Error::new(ErrorCode::InvalidParams.into(), "denied");
    let ec_err = ElicitationCreateError::ClientError(client_err);
    assert!(ec_err.source().is_some());
}

#[tokio::test]
async fn handle_success_publishes_response_to_reply_subject() {
    let nats = MockNatsClient::new();
    let client = MockClient::new(ElicitationAcceptAction::new());
    let request = session_scoped_request("session-001");
    let (headers, payload) = make_wire_request(request);

    handle(&headers, &payload, &client, Some("_INBOX.reply"), &nats, "session-001").await;

    assert_eq!(nats.published_messages(), vec!["_INBOX.reply"]);
}

#[tokio::test]
async fn handle_no_reply_does_not_publish() {
    let nats = MockNatsClient::new();
    let client = MockClient::new(ElicitationAcceptAction::new());
    let request = session_scoped_request("session-001");
    let (headers, payload) = make_wire_request(request);

    handle(&headers, &payload, &client, None, &nats, "session-001").await;

    assert!(nats.published_messages().is_empty());
}

#[tokio::test]
async fn handle_session_id_mismatch_publishes_error_reply() {
    let nats = MockNatsClient::new();
    let client = MockClient::new(ElicitationAcceptAction::new());
    let request = session_scoped_request("session-other");
    let (headers, payload) = make_wire_request(request);

    handle(&headers, &payload, &client, Some("_INBOX.err"), &nats, "session-001").await;

    assert_eq!(nats.published_messages(), vec!["_INBOX.err"]);
}

#[tokio::test]
async fn handle_invalid_payload_publishes_error_reply() {
    let nats = MockNatsClient::new();
    let client = MockClient::new(ElicitationAcceptAction::new());

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
    let request = session_scoped_request("session-001");
    let (headers, payload) = make_wire_request(request);

    handle(&headers, &payload, &client, Some("_INBOX.err"), &nats, "session-001").await;

    assert_eq!(nats.published_messages(), vec!["_INBOX.err"]);
}

#[tokio::test]
async fn handle_success_flush_failure_exercises_warn_path() {
    let nats = AdvancedMockNatsClient::new();
    nats.fail_next_flush();
    let client = MockClient::new(ElicitationAcceptAction::new());
    let request = session_scoped_request("session-001");
    let (headers, payload) = make_wire_request(request);

    handle(&headers, &payload, &client, Some("_INBOX.reply"), &nats, "session-001").await;

    assert_eq!(nats.published_messages(), vec!["_INBOX.reply"]);
}

#[tokio::test]
async fn handle_success_publish_failure_exercises_error_path() {
    let nats = AdvancedMockNatsClient::new();
    nats.fail_next_publish();
    let client = MockClient::new(ElicitationAcceptAction::new());
    let request = session_scoped_request("session-001");
    let (headers, payload) = make_wire_request(request);

    handle(&headers, &payload, &client, Some("_INBOX.reply"), &nats, "session-001").await;

    assert!(nats.published_messages().is_empty());
}
