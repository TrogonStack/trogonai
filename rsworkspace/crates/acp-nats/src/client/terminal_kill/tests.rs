use super::super::tests::{MockClient, TerminalKillFailingClient};
use super::*;
use async_nats::header::HeaderMap;
use jsonrpc_nats::RequestId;
use std::error::Error;
use trogon_nats::{AdvancedMockNatsClient, MockNatsClient};

fn empty_headers() -> HeaderMap {
    HeaderMap::new()
}

fn make_wire_request<T: serde::Serialize>(params: &T) -> (HeaderMap, Vec<u8>) {
    crate::client::test_support::encode_wire_request("terminal/kill", RequestId::Number(1), params)
}

#[tokio::test]
async fn handle_success_publishes_response_to_reply_subject() {
    let nats = MockNatsClient::new();
    let client = MockClient::new();
    let request = KillTerminalRequest::new(agent_client_protocol::SessionId::from("sess-1"), "term-001".to_string());
    let (headers, payload) = make_wire_request(&request);

    handle(&headers, &payload, &client, Some("_INBOX.reply"), &nats, "sess-1").await;

    assert_eq!(nats.published_messages(), vec!["_INBOX.reply"]);
    let published_headers = nats.published_headers()[0].clone();
    assert_eq!(published_headers.get(jsonrpc_nats::HEADER_ID).unwrap().as_str(), "1");
    assert!(published_headers.get(jsonrpc_nats::HEADER_ERROR_CODE).is_none());
}

#[tokio::test]
async fn handle_no_reply_does_not_call_client_or_publish() {
    let nats = MockNatsClient::new();
    let client = MockClient::new();
    let request = KillTerminalRequest::new(agent_client_protocol::SessionId::from("sess-1"), "term-001".to_string());
    let (headers, payload) = make_wire_request(&request);

    handle(&headers, &payload, &client, None, &nats, "sess-1").await;

    assert!(nats.published_messages().is_empty());
    assert_eq!(client.kill_terminal_call_count(), 0);
}

#[tokio::test]
async fn handle_invalid_json_publishes_parse_error() {
    let nats = MockNatsClient::new();
    let client = MockClient::new();

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

#[tokio::test]
async fn handle_session_id_mismatch_publishes_error_reply() {
    let nats = MockNatsClient::new();
    let client = MockClient::new();
    let request = KillTerminalRequest::new(agent_client_protocol::SessionId::from("sess-b"), "term-001".to_string());
    let (headers, payload) = make_wire_request(&request);

    handle(&headers, &payload, &client, Some("_INBOX.err"), &nats, "sess-a").await;

    assert_eq!(nats.published_messages(), vec!["_INBOX.err"]);
}

#[tokio::test]
async fn handle_client_error_publishes_error_reply() {
    let nats = MockNatsClient::new();
    let client = TerminalKillFailingClient;
    let request = KillTerminalRequest::new(agent_client_protocol::SessionId::from("sess-1"), "term-001".to_string());
    let (headers, payload) = make_wire_request(&request);

    handle(&headers, &payload, &client, Some("_INBOX.err"), &nats, "sess-1").await;

    assert_eq!(nats.published_messages(), vec!["_INBOX.err"]);
    let published_headers = nats.published_headers()[0].clone();
    assert!(published_headers.get(jsonrpc_nats::HEADER_ERROR_CODE).is_some());
    let payloads = nats.published_payloads();
    let body: serde_json::Value = serde_json::from_slice(payloads[0].as_ref()).unwrap();
    assert_eq!(body["message"], "mock kill_terminal failure");
}

#[tokio::test]
async fn handle_success_publish_failure_exercises_error_path() {
    let nats = AdvancedMockNatsClient::new();
    nats.fail_next_publish();
    let client = MockClient::new();
    let request = KillTerminalRequest::new(agent_client_protocol::SessionId::from("sess-1"), "term-001".to_string());
    let (headers, payload) = make_wire_request(&request);

    handle(&headers, &payload, &client, Some("_INBOX.reply"), &nats, "sess-1").await;

    assert!(nats.published_messages().is_empty());
}

#[tokio::test]
async fn handle_success_flush_failure_exercises_warn_path() {
    let nats = AdvancedMockNatsClient::new();
    nats.fail_next_flush();
    let client = MockClient::new();
    let request = KillTerminalRequest::new(agent_client_protocol::SessionId::from("sess-1"), "term-001".to_string());
    let (headers, payload) = make_wire_request(&request);

    handle(&headers, &payload, &client, Some("_INBOX.reply"), &nats, "sess-1").await;

    assert_eq!(nats.published_messages(), vec!["_INBOX.reply"]);
}

#[test]
fn error_code_and_message_client_error_preserves_client_code() {
    let err = TerminalKillError::ClientError(agent_client_protocol::Error::new(
        ErrorCode::InvalidParams.into(),
        "bad terminal id",
    ));
    let (code, message) = error_code_and_message(&err);
    assert_eq!(code, ErrorCode::InvalidParams);
    assert_eq!(message, "bad terminal id");
}

#[test]
fn error_code_and_message_invalid_params() {
    let err = TerminalKillError::InvalidParams(agent_client_protocol::Error::new(
        ErrorCode::InvalidParams.into(),
        "bad params",
    ));
    let (code, message) = error_code_and_message(&err);
    assert_eq!(code, ErrorCode::InvalidParams);
    assert_eq!(message, "bad params");
}

#[test]
fn terminal_kill_error_display() {
    let invalid_params = TerminalKillError::InvalidParams(agent_client_protocol::Error::new(
        ErrorCode::InvalidParams.into(),
        "bad params",
    ));
    assert_eq!(invalid_params.to_string(), "invalid params: bad params");

    let client_err = TerminalKillError::ClientError(agent_client_protocol::Error::new(
        ErrorCode::InvalidParams.into(),
        "client fail",
    ));
    assert_eq!(client_err.to_string(), "client error: client fail");
}

#[test]
fn terminal_kill_error_source() {
    let invalid_params = TerminalKillError::InvalidParams(agent_client_protocol::Error::new(
        ErrorCode::InvalidParams.into(),
        "bad params",
    ));
    assert!(invalid_params.source().is_some());

    let client_err = TerminalKillError::ClientError(agent_client_protocol::Error::new(
        ErrorCode::InvalidParams.into(),
        "client fail",
    ));
    assert!(client_err.source().is_some());
}
