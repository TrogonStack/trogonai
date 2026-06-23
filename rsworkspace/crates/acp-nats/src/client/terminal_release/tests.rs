use super::*;
use agent_client_protocol::{
    ContentBlock, ContentChunk, Request, RequestId, RequestPermissionRequest, RequestPermissionResponse,
    SessionNotification, SessionUpdate,
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

    async fn release_terminal(
        &self,
        _: ReleaseTerminalRequest,
    ) -> agent_client_protocol::Result<ReleaseTerminalResponse> {
        Ok(ReleaseTerminalResponse::new())
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

    async fn release_terminal(
        &self,
        _: ReleaseTerminalRequest,
    ) -> agent_client_protocol::Result<ReleaseTerminalResponse> {
        Err(agent_client_protocol::Error::new(
            -32603,
            "mock release_terminal failure",
        ))
    }
}

#[tokio::test]
async fn handle_success_publishes_response_to_reply_subject() {
    let nats = MockNatsClient::new();
    let client = MockClient::new();
    let request = ReleaseTerminalRequest::new("sess-1", "term-001");
    let envelope = Request {
        id: RequestId::Number(1),
        method: std::sync::Arc::from("terminal/release"),
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
    let request = ReleaseTerminalRequest::new("sess-1", "term-001");
    let envelope = Request {
        id: RequestId::Number(1),
        method: std::sync::Arc::from("terminal/release"),
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
    let payload = br#"{"id":1,"method":"terminal/release","params":{}}"#;

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
    let request = ReleaseTerminalRequest::new("sess-1", "term-001");
    let envelope = Request {
        id: RequestId::Number(1),
        method: std::sync::Arc::from("terminal/release"),
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
    let request = ReleaseTerminalRequest::new("sess-b", "term-001");
    let envelope = Request {
        id: RequestId::Number(1),
        method: std::sync::Arc::from("terminal/release"),
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
    let request = ReleaseTerminalRequest::new("sess-1", "term-001");
    let envelope = Request {
        id: RequestId::Number(1),
        method: std::sync::Arc::from("terminal/release"),
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
    let request = ReleaseTerminalRequest::new("sess-1", "term-001");
    let envelope = Request {
        id: RequestId::Number(1),
        method: std::sync::Arc::from("terminal/release"),
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
    let request = ReleaseTerminalRequest::new("sess-1", "term-001");
    let envelope = Request {
        id: RequestId::Number(1),
        method: std::sync::Arc::from("terminal/release"),
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
    let tr_err = TerminalReleaseError::MalformedJson(err);
    let (code, message) = error_code_and_message(&tr_err);
    assert_eq!(code, ErrorCode::ParseError);
    assert!(message.contains("Malformed terminal/release request JSON"));
}

#[test]
fn error_code_and_message_invalid_params_preserves_code_and_message() {
    let inner = agent_client_protocol::Error::new(ErrorCode::InvalidParams.into(), "params is null");
    let tr_err = TerminalReleaseError::InvalidParams(inner);
    let (code, message) = error_code_and_message(&tr_err);
    assert_eq!(code, ErrorCode::InvalidParams);
    assert_eq!(message, "params is null");
}

#[test]
fn error_code_and_message_client_error_preserves_client_code() {
    let client_err = agent_client_protocol::Error::new(ErrorCode::InvalidParams.into(), "denied");
    let tr_err = TerminalReleaseError::ClientError(client_err);
    let (code, message) = error_code_and_message(&tr_err);
    assert_eq!(code, ErrorCode::InvalidParams);
    assert_eq!(message, "denied");
}

#[test]
fn terminal_release_error_display() {
    let malformed =
        TerminalReleaseError::MalformedJson(serde_json::from_slice::<serde_json::Value>(b"not json").unwrap_err());
    assert!(malformed.to_string().contains("malformed JSON"));

    let invalid_params = TerminalReleaseError::InvalidParams(agent_client_protocol::Error::new(
        ErrorCode::InvalidParams.into(),
        "bad params",
    ));
    assert!(invalid_params.to_string().contains("invalid params"));

    let client_err = TerminalReleaseError::ClientError(agent_client_protocol::Error::new(
        ErrorCode::InvalidParams.into(),
        "client fail",
    ));
    assert!(client_err.to_string().contains("client error"));
}

#[test]
fn terminal_release_error_source() {
    let malformed =
        TerminalReleaseError::MalformedJson(serde_json::from_slice::<serde_json::Value>(b"not json").unwrap_err());
    assert!(malformed.source().is_some());

    let invalid_params = TerminalReleaseError::InvalidParams(agent_client_protocol::Error::new(
        ErrorCode::InvalidParams.into(),
        "bad params",
    ));
    assert!(invalid_params.source().is_some());

    let client_err = TerminalReleaseError::ClientError(agent_client_protocol::Error::new(
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
