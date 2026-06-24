use super::super::tests::{MockClient, TerminalKillFailingClient};
use super::*;
use std::error::Error;
use trogon_nats::{AdvancedMockNatsClient, MockNatsClient};
use trogon_std::{FailNextSerialize, StdJsonSerialize};

#[tokio::test]
async fn handle_success_publishes_response_to_reply_subject() {
    let nats = MockNatsClient::new();
    let client = MockClient::new();
    let request = KillTerminalRequest::new(agent_client_protocol::SessionId::from("sess-1"), "term-001".to_string());
    let envelope = Request {
        id: agent_client_protocol::RequestId::Number(1),
        method: std::sync::Arc::from("terminal/kill"),
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
    let payloads = nats.published_payloads();
    assert_eq!(payloads.len(), 1);
    let parsed: serde_json::Value = serde_json::from_slice(payloads[0].as_ref()).unwrap();
    assert_eq!(parsed.get("id"), Some(&serde_json::Value::from(1)));
    assert!(parsed.get("result").is_some());
}

#[tokio::test]
async fn handle_no_reply_does_not_call_client_or_publish() {
    let nats = MockNatsClient::new();
    let client = MockClient::new();
    let request = KillTerminalRequest::new(agent_client_protocol::SessionId::from("sess-1"), "term-001".to_string());
    let envelope = Request {
        id: agent_client_protocol::RequestId::Number(1),
        method: std::sync::Arc::from("terminal/kill"),
        params: Some(request),
    };
    let payload = serde_json::to_vec(&envelope).unwrap();

    handle(&payload, &client, None, &nats, "sess-1", &StdJsonSerialize).await;

    assert!(nats.published_messages().is_empty());
    assert_eq!(client.kill_terminal_call_count(), 0);
}

#[tokio::test]
async fn handle_invalid_json_publishes_parse_error() {
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
    assert_eq!(payloads.len(), 1);
    let response: serde_json::Value = serde_json::from_slice(payloads[0].as_ref()).unwrap();
    assert_eq!(
        response.get("error").and_then(|e| e.get("code")),
        Some(&serde_json::Value::from(-32700))
    );
}

#[tokio::test]
async fn handle_session_id_mismatch_publishes_error_reply() {
    let nats = MockNatsClient::new();
    let client = MockClient::new();
    let request = KillTerminalRequest::new(agent_client_protocol::SessionId::from("sess-b"), "term-001".to_string());
    let envelope = Request {
        id: agent_client_protocol::RequestId::Number(1),
        method: std::sync::Arc::from("terminal/kill"),
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
async fn handle_client_error_publishes_error_reply() {
    let nats = MockNatsClient::new();
    let client = TerminalKillFailingClient;
    let request = KillTerminalRequest::new(agent_client_protocol::SessionId::from("sess-1"), "term-001".to_string());
    let envelope = Request {
        id: agent_client_protocol::RequestId::Number(1),
        method: std::sync::Arc::from("terminal/kill"),
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
    let payloads = nats.published_payloads();
    assert_eq!(payloads.len(), 1);
    let response: serde_json::Value = serde_json::from_slice(payloads[0].as_ref()).unwrap();
    assert_eq!(
        response.get("error").and_then(|e| e.get("message")),
        Some(&serde_json::Value::from("mock kill_terminal failure"))
    );
}

#[tokio::test]
async fn handle_success_serialization_fallback_sends_error_reply() {
    let nats = MockNatsClient::new();
    let client = MockClient::new();
    let serializer = FailNextSerialize::new(1);
    let request = KillTerminalRequest::new(agent_client_protocol::SessionId::from("sess-1"), "term-001".to_string());
    let envelope = Request {
        id: agent_client_protocol::RequestId::Number(1),
        method: std::sync::Arc::from("terminal/kill"),
        params: Some(request),
    };
    let payload = serde_json::to_vec(&envelope).unwrap();

    handle(&payload, &client, Some("_INBOX.reply"), &nats, "sess-1", &serializer).await;

    assert_eq!(nats.published_messages(), vec!["_INBOX.reply"]);
}

#[tokio::test]
async fn handle_success_publish_failure_exercises_error_path() {
    let nats = AdvancedMockNatsClient::new();
    nats.fail_next_publish();
    let client = MockClient::new();
    let request = KillTerminalRequest::new(agent_client_protocol::SessionId::from("sess-1"), "term-001".to_string());
    let envelope = Request {
        id: agent_client_protocol::RequestId::Number(1),
        method: std::sync::Arc::from("terminal/kill"),
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
    let request = KillTerminalRequest::new(agent_client_protocol::SessionId::from("sess-1"), "term-001".to_string());
    let envelope = Request {
        id: agent_client_protocol::RequestId::Number(1),
        method: std::sync::Arc::from("terminal/kill"),
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
fn error_code_and_message_malformed_json_returns_parse_error() {
    let err = TerminalKillError::MalformedJson(serde_json::from_slice::<serde_json::Value>(b"not json").unwrap_err());
    let (code, message) = error_code_and_message(&err);
    assert_eq!(code, ErrorCode::ParseError);
    assert!(message.contains("Malformed terminal/kill request JSON"));
}

#[test]
fn terminal_kill_error_display() {
    let json_err = serde_json::from_slice::<serde_json::Value>(b"not json").unwrap_err();
    let expected = format!("malformed JSON: {json_err}");
    let malformed = TerminalKillError::MalformedJson(json_err);
    assert_eq!(malformed.to_string(), expected);

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
    let malformed =
        TerminalKillError::MalformedJson(serde_json::from_slice::<serde_json::Value>(b"not json").unwrap_err());
    assert!(malformed.source().is_some());

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
