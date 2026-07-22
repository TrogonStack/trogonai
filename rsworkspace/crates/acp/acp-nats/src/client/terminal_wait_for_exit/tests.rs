use super::super::tests::{MockClient, TerminalWaitForExitFailingClient, TerminalWaitForExitTimeoutClient};
use super::*;
use agent_client_protocol::schema::v1::WaitForTerminalExitRequest;
use async_nats::header::HeaderMap;
use jsonrpc_nats::RequestId;
use std::error::Error;
use std::time::Duration;
use trogon_nats::{AdvancedMockNatsClient, MockNatsClient};

fn empty_headers() -> HeaderMap {
    HeaderMap::new()
}

fn make_wire_request<T: serde::Serialize>(params: &T) -> (HeaderMap, Vec<u8>) {
    crate::client::test_support::encode_wire_request("terminal/wait_for_exit", RequestId::Number(1), params)
}

fn sample_request() -> WaitForTerminalExitRequest {
    WaitForTerminalExitRequest::new("sess-1", "term-001")
}

#[tokio::test]
async fn handle_success_publishes_response_to_reply_subject() {
    let nats = MockNatsClient::new();
    let client = MockClient::new();
    let request = sample_request();
    let (headers, payload) = make_wire_request(&request);

    handle(
        &headers,
        &payload,
        &client,
        Some("_INBOX.reply"),
        &nats,
        "sess-1",
        Duration::from_secs(5),
    )
    .await;

    assert_eq!(nats.published_messages(), vec!["_INBOX.reply"]);
    let published_headers = nats.published_headers()[0].clone();
    assert_eq!(published_headers.get(jsonrpc_nats::HEADER_ID).unwrap().as_str(), "1");
    assert!(published_headers.get(jsonrpc_nats::HEADER_ERROR_CODE).is_none());
}

#[tokio::test]
async fn handle_no_reply_does_not_call_client_or_publish() {
    let nats = MockNatsClient::new();
    let client = MockClient::new();
    let request = sample_request();
    let (headers, payload) = make_wire_request(&request);

    handle(
        &headers,
        &payload,
        &client,
        None,
        &nats,
        "sess-1",
        Duration::from_secs(5),
    )
    .await;

    assert!(nats.published_messages().is_empty());
    assert_eq!(client.wait_for_terminal_exit_call_count(), 0);
}

#[tokio::test]
async fn handle_malformed_json_publishes_parse_error() {
    let nats = MockNatsClient::new();
    let client = MockClient::new();

    handle(
        &empty_headers(),
        b"not json",
        &client,
        Some("_INBOX.err"),
        &nats,
        "sess-1",
        Duration::from_secs(5),
    )
    .await;

    assert_eq!(nats.published_messages(), vec!["_INBOX.err"]);
    let published_headers = nats.published_headers()[0].clone();
    assert!(published_headers.get(jsonrpc_nats::HEADER_ERROR_CODE).is_some());
}

#[tokio::test]
async fn handle_invalid_params_publishes_error_reply() {
    let nats = MockNatsClient::new();
    let client = MockClient::new();
    let payload = br#"{"id":1,"method":"terminal/wait_for_exit","params":{}}"#;

    handle(
        &empty_headers(),
        payload,
        &client,
        Some("_INBOX.err"),
        &nats,
        "sess-1",
        Duration::from_secs(5),
    )
    .await;

    assert_eq!(nats.published_messages(), vec!["_INBOX.err"]);
}

#[tokio::test]
async fn handle_params_null_publishes_error_reply() {
    let nats = MockNatsClient::new();
    let client = MockClient::new();
    let payload = br#"{"id":1,"method":"terminal/wait_for_exit","params":null}"#;

    handle(
        &empty_headers(),
        payload,
        &client,
        Some("_INBOX.err"),
        &nats,
        "sess-1",
        Duration::from_secs(5),
    )
    .await;

    assert_eq!(nats.published_messages(), vec!["_INBOX.err"]);
}

#[tokio::test]
async fn handle_session_id_mismatch_publishes_error_reply() {
    let nats = MockNatsClient::new();
    let client = MockClient::new();
    let request = sample_request();
    let (headers, payload) = make_wire_request(&request);

    handle(
        &headers,
        &payload,
        &client,
        Some("_INBOX.err"),
        &nats,
        "sess-a",
        Duration::from_secs(5),
    )
    .await;

    assert_eq!(nats.published_messages(), vec!["_INBOX.err"]);
    assert_eq!(client.wait_for_terminal_exit_call_count(), 0);
}

#[tokio::test]
async fn handle_client_error_publishes_error_reply() {
    let nats = MockNatsClient::new();
    let client = TerminalWaitForExitFailingClient;
    let request = sample_request();
    let (headers, payload) = make_wire_request(&request);

    handle(
        &headers,
        &payload,
        &client,
        Some("_INBOX.err"),
        &nats,
        "sess-1",
        Duration::from_secs(5),
    )
    .await;

    assert_eq!(nats.published_messages(), vec!["_INBOX.err"]);
    let published_headers = nats.published_headers()[0].clone();
    assert!(published_headers.get(jsonrpc_nats::HEADER_ERROR_CODE).is_some());
    let payloads = nats.published_payloads();
    let body: serde_json::Value = serde_json::from_slice(payloads[0].as_ref()).unwrap();
    assert!(
        body["message"]
            .as_str()
            .unwrap()
            .contains("mock wait_for_terminal_exit failure")
    );
}

#[tokio::test]
async fn handle_timeout_publishes_error_reply() {
    let nats = MockNatsClient::new();
    let client = TerminalWaitForExitTimeoutClient;
    let request = sample_request();
    let (headers, payload) = make_wire_request(&request);

    handle(
        &headers,
        &payload,
        &client,
        Some("_INBOX.err"),
        &nats,
        "sess-1",
        Duration::from_millis(10),
    )
    .await;

    assert_eq!(nats.published_messages(), vec!["_INBOX.err"]);
    let published_headers = nats.published_headers()[0].clone();
    assert!(published_headers.get(jsonrpc_nats::HEADER_ERROR_CODE).is_some());
}

#[tokio::test]
async fn handle_success_publish_failure_exercises_error_path() {
    let nats = AdvancedMockNatsClient::new();
    nats.fail_next_publish();
    let client = MockClient::new();
    let request = sample_request();
    let (headers, payload) = make_wire_request(&request);

    handle(
        &headers,
        &payload,
        &client,
        Some("_INBOX.reply"),
        &nats,
        "sess-1",
        Duration::from_secs(5),
    )
    .await;

    assert!(nats.published_messages().is_empty());
}

#[tokio::test]
async fn handle_success_flush_failure_exercises_warn_path() {
    let nats = AdvancedMockNatsClient::new();
    nats.fail_next_flush();
    let client = MockClient::new();
    let request = sample_request();
    let (headers, payload) = make_wire_request(&request);

    handle(
        &headers,
        &payload,
        &client,
        Some("_INBOX.reply"),
        &nats,
        "sess-1",
        Duration::from_secs(5),
    )
    .await;

    assert_eq!(nats.published_messages(), vec!["_INBOX.reply"]);
}

#[test]
fn error_code_and_message_invalid_params() {
    let err = TerminalWaitForExitError::InvalidParams(agent_client_protocol::Error::new(-32602, "bad params"));
    let (code, msg) = error_code_and_message(&err);
    assert_eq!(i32::from(code), -32602);
    assert_eq!(msg, "bad params");
}

#[test]
fn error_code_and_message_timed_out() {
    let err = TerminalWaitForExitError::TimedOut;
    let (code, msg) = error_code_and_message(&err);
    assert_eq!(code, ErrorCode::InternalError);
    assert_eq!(msg, "Timed out waiting for terminal exit");
}

#[test]
fn error_code_and_message_client_error() {
    let err = TerminalWaitForExitError::ClientError(agent_client_protocol::Error::new(-32603, "client failed"));
    let (code, msg) = error_code_and_message(&err);
    assert_eq!(i32::from(code), -32603);
    assert_eq!(msg, "client failed");
}

#[test]
fn terminal_wait_for_exit_error_display() {
    let err = TerminalWaitForExitError::TimedOut;
    assert_eq!(err.to_string(), "Timed out waiting for terminal exit");

    let params_err = TerminalWaitForExitError::InvalidParams(agent_client_protocol::Error::new(-32602, "bad params"));
    assert_eq!(params_err.to_string(), "invalid params: bad params");

    let client_err = TerminalWaitForExitError::ClientError(agent_client_protocol::Error::new(-32603, "client failed"));
    assert_eq!(client_err.to_string(), "client error: client failed");
}

#[test]
fn terminal_wait_for_exit_error_source() {
    let err = TerminalWaitForExitError::TimedOut;
    assert!(err.source().is_none());

    let params_err = TerminalWaitForExitError::InvalidParams(agent_client_protocol::Error::new(-32602, "bad params"));
    assert!(params_err.source().is_some());

    let client_err = TerminalWaitForExitError::ClientError(agent_client_protocol::Error::new(-32603, "client failed"));
    assert!(client_err.source().is_some());
}
