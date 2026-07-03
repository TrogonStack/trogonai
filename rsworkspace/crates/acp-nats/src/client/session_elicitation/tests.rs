use super::*;
use agent_client_protocol::{ElicitationFormMode, ElicitationMode, ElicitationSchema};
use jsonrpc_nats::RequestId;
use std::error::Error;
use trogon_nats::{AdvancedMockNatsClient, MockNatsClient};

struct DefaultClient;

fn make_request(session_id: &str) -> ElicitationRequest {
    ElicitationRequest::new(
        session_id.to_string(),
        ElicitationMode::Form(ElicitationFormMode::new(ElicitationSchema::new())),
        "Please provide input",
    )
}

fn make_wire_request(request: ElicitationRequest) -> (HeaderMap, Vec<u8>) {
    crate::client::test_support::encode_wire_request("session/elicitation", RequestId::Number(1), &request)
}

fn empty_headers() -> HeaderMap {
    HeaderMap::new()
}

#[tokio::test]
async fn default_client_returns_cancel() {
    let client = DefaultClient;
    let request = make_request("sess-1");
    let result = client.request_elicitation(request).await;
    assert!(result.is_ok());
    assert!(matches!(result.unwrap().action, ElicitationAction::Cancel));
}

#[tokio::test]
async fn request_elicitation_forwards_request_and_returns_response() {
    let client = DefaultClient;
    let request = make_request("session-001");
    let (headers, payload) = make_wire_request(request);

    let result = forward_to_client(&headers, &payload, &client, "session-001").await;
    assert!(result.is_ok());
    assert!(matches!(result.unwrap().action, ElicitationAction::Cancel));
}

#[tokio::test]
async fn request_elicitation_returns_error_when_payload_is_invalid_json() {
    let client = DefaultClient;
    let result = forward_to_client(&empty_headers(), b"not json", &client, "session-001").await;
    assert!(result.is_err());
}

#[tokio::test]
async fn request_elicitation_returns_invalid_request_when_params_missing() {
    let client = DefaultClient;
    let result = forward_to_client(&empty_headers(), b"{}", &client, "session-001").await;
    assert!(result.is_err());
    assert!(matches!(
        result.unwrap_err(),
        SessionElicitationError::InvalidRequest(_)
    ));
}

#[tokio::test]
async fn request_elicitation_returns_invalid_request_when_session_id_mismatch() {
    let client = DefaultClient;
    let request = make_request("session-other");
    let (headers, payload) = make_wire_request(request);

    let result = forward_to_client(&headers, &payload, &client, "session-001").await;
    assert!(result.is_err());
    assert!(matches!(
        result.unwrap_err(),
        SessionElicitationError::InvalidRequest(_)
    ));
}

#[test]
fn error_code_and_message_invalid_request_returns_invalid_params() {
    let err = serde_json::from_slice::<ElicitationRequest>(b"not json").unwrap_err();
    let se_err = SessionElicitationError::InvalidRequest(err);
    let (code, _) = error_code_and_message(&se_err);
    assert_eq!(code, ErrorCode::InvalidParams);
}

#[test]
fn error_code_and_message_client_error_preserves_client_code() {
    let client_err = agent_client_protocol::Error::new(ErrorCode::InvalidParams.into(), "denied");
    let se_err = SessionElicitationError::ClientError(client_err);
    let (code, message) = error_code_and_message(&se_err);
    assert_eq!(code, ErrorCode::InvalidParams);
    assert_eq!(message, "denied");
}

#[test]
fn session_elicitation_error_display() {
    let err = serde_json::from_slice::<ElicitationRequest>(b"not json").unwrap_err();
    let expected = format!("invalid request: {err}");
    let se_err = SessionElicitationError::InvalidRequest(err);
    assert_eq!(se_err.to_string(), expected);

    let client_err = agent_client_protocol::Error::new(ErrorCode::InvalidParams.into(), "elicitation denied");
    let se_err = SessionElicitationError::ClientError(client_err);
    assert_eq!(se_err.to_string(), "client error: elicitation denied");
}

#[test]
fn session_elicitation_error_source() {
    let err = serde_json::from_slice::<ElicitationRequest>(b"not json").unwrap_err();
    let se_err = SessionElicitationError::InvalidRequest(err);
    assert!(se_err.source().is_some());

    let client_err = agent_client_protocol::Error::new(ErrorCode::InvalidParams.into(), "denied");
    let se_err = SessionElicitationError::ClientError(client_err);
    assert!(se_err.source().is_some());
}

#[tokio::test]
async fn handle_success_publishes_response_to_reply_subject() {
    let nats = MockNatsClient::new();
    let client = DefaultClient;
    let request = make_request("session-001");
    let (headers, payload) = make_wire_request(request);

    handle(&headers, &payload, &client, Some("_INBOX.reply"), &nats, "session-001").await;

    assert_eq!(nats.published_messages(), vec!["_INBOX.reply"]);
}

#[tokio::test]
async fn handle_no_reply_does_not_publish() {
    let nats = MockNatsClient::new();
    let client = DefaultClient;
    let request = make_request("session-001");
    let (headers, payload) = make_wire_request(request);

    handle(&headers, &payload, &client, None, &nats, "session-001").await;

    assert!(nats.published_messages().is_empty());
}

#[tokio::test]
async fn handle_session_id_mismatch_publishes_error_reply() {
    let nats = MockNatsClient::new();
    let client = DefaultClient;
    let request = make_request("session-other");
    let (headers, payload) = make_wire_request(request);

    handle(&headers, &payload, &client, Some("_INBOX.err"), &nats, "session-001").await;

    assert_eq!(nats.published_messages(), vec!["_INBOX.err"]);
}

#[tokio::test]
async fn handle_invalid_payload_publishes_error_reply() {
    let nats = MockNatsClient::new();
    let client = DefaultClient;

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
async fn handle_success_flush_failure_exercises_warn_path() {
    let nats = AdvancedMockNatsClient::new();
    nats.fail_next_flush();
    let client = DefaultClient;
    let request = make_request("session-001");
    let (headers, payload) = make_wire_request(request);

    handle(&headers, &payload, &client, Some("_INBOX.reply"), &nats, "session-001").await;

    assert_eq!(nats.published_messages(), vec!["_INBOX.reply"]);
}

#[tokio::test]
async fn handle_success_publish_failure_exercises_error_path() {
    let nats = AdvancedMockNatsClient::new();
    nats.fail_next_publish();
    let client = DefaultClient;
    let request = make_request("session-001");
    let (headers, payload) = make_wire_request(request);

    handle(&headers, &payload, &client, Some("_INBOX.reply"), &nats, "session-001").await;

    assert!(nats.published_messages().is_empty());
}
