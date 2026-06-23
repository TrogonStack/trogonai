use super::*;
use agent_client_protocol::{
    ExtRequest, ExtResponse, RequestId, RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse,
    SessionNotification,
};
use async_trait::async_trait;
use std::cell::RefCell;
use std::error::Error;
use trogon_nats::{AdvancedMockNatsClient, MockNatsClient};
use trogon_std::{FailNextSerialize, StdJsonSerialize};

struct MockClient {
    notifications: RefCell<Vec<String>>,
}

impl MockClient {
    fn new() -> Self {
        Self {
            notifications: RefCell::new(Vec::new()),
        }
    }

    fn notification_count(&self) -> usize {
        self.notifications.borrow().len()
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
        Ok(RequestPermissionResponse::new(RequestPermissionOutcome::Cancelled))
    }

    async fn ext_method(&self, _: ExtRequest) -> agent_client_protocol::Result<ExtResponse> {
        let raw = RawValue::from_string(r#"{"status":"ok"}"#.to_string()).unwrap();
        Ok(ExtResponse::new(raw.into()))
    }

    async fn ext_notification(&self, args: ExtNotification) -> agent_client_protocol::Result<()> {
        self.notifications.borrow_mut().push(args.method.to_string());
        Ok(())
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
        Ok(RequestPermissionResponse::new(RequestPermissionOutcome::Cancelled))
    }

    async fn ext_method(&self, _: ExtRequest) -> agent_client_protocol::Result<ExtResponse> {
        Err(agent_client_protocol::Error::new(
            ErrorCode::InternalError.into(),
            "ext method failed",
        ))
    }

    async fn ext_notification(&self, _: ExtNotification) -> agent_client_protocol::Result<()> {
        Err(agent_client_protocol::Error::new(-1, "notification failed"))
    }
}

fn make_ext_envelope(params_json: &str) -> Vec<u8> {
    let raw = RawValue::from_string(params_json.to_string()).unwrap();
    let envelope = Request {
        id: RequestId::Number(1),
        method: Arc::from("_my_method"),
        params: Some(Arc::<RawValue>::from(raw)),
    };
    serde_json::to_vec(&envelope).unwrap()
}

// --- request/response tests ---

#[tokio::test]
async fn request_publishes_response_to_reply_subject() {
    let nats = MockNatsClient::new();
    let client = MockClient::new();
    let payload = make_ext_envelope(r#"{"key":"value"}"#);

    handle(
        &payload,
        &client,
        Some("_INBOX.reply"),
        &nats,
        "my_method",
        &StdJsonSerialize,
    )
    .await;

    assert_eq!(nats.published_messages(), vec!["_INBOX.reply"]);
    let payloads = nats.published_payloads();
    let parsed: serde_json::Value = serde_json::from_slice(payloads[0].as_ref()).unwrap();
    assert_eq!(parsed.get("id"), Some(&serde_json::Value::from(1)));
    assert!(parsed.get("result").is_some());
}

#[tokio::test]
async fn request_invalid_payload_publishes_error_reply() {
    let nats = MockNatsClient::new();
    let client = MockClient::new();

    handle(
        b"not json",
        &client,
        Some("_INBOX.err"),
        &nats,
        "my_method",
        &StdJsonSerialize,
    )
    .await;

    assert_eq!(nats.published_messages(), vec!["_INBOX.err"]);
    let payloads = nats.published_payloads();
    let parsed: serde_json::Value = serde_json::from_slice(payloads[0].as_ref()).unwrap();
    assert_eq!(
        parsed.get("error").and_then(|e| e.get("code")),
        Some(&serde_json::Value::from(-32700))
    );
}

#[tokio::test]
async fn request_client_error_publishes_error_reply() {
    let nats = MockNatsClient::new();
    let client = FailingClient;
    let payload = make_ext_envelope(r#"{}"#);

    handle(
        &payload,
        &client,
        Some("_INBOX.err"),
        &nats,
        "my_method",
        &StdJsonSerialize,
    )
    .await;

    assert_eq!(nats.published_messages(), vec!["_INBOX.err"]);
    let payloads = nats.published_payloads();
    let parsed: serde_json::Value = serde_json::from_slice(payloads[0].as_ref()).unwrap();
    assert_eq!(
        parsed.get("error").and_then(|e| e.get("message")),
        Some(&serde_json::Value::from("ext method failed"))
    );
}

#[tokio::test]
async fn request_serialization_fallback_sends_error_reply() {
    let nats = MockNatsClient::new();
    let client = MockClient::new();
    let serializer = FailNextSerialize::new(1);
    let payload = make_ext_envelope(r#"{}"#);

    handle(&payload, &client, Some("_INBOX.reply"), &nats, "my_method", &serializer).await;

    assert_eq!(nats.published_messages(), vec!["_INBOX.reply"]);
}

#[tokio::test]
async fn request_publish_failure_exercises_error_path() {
    let nats = AdvancedMockNatsClient::new();
    nats.fail_next_publish();
    let client = MockClient::new();
    let payload = make_ext_envelope(r#"{}"#);

    handle(
        &payload,
        &client,
        Some("_INBOX.reply"),
        &nats,
        "my_method",
        &StdJsonSerialize,
    )
    .await;

    assert!(nats.published_messages().is_empty());
}

#[tokio::test]
async fn request_flush_failure_exercises_warn_path() {
    let nats = AdvancedMockNatsClient::new();
    nats.fail_next_flush();
    let client = MockClient::new();
    let payload = make_ext_envelope(r#"{}"#);

    handle(
        &payload,
        &client,
        Some("_INBOX.reply"),
        &nats,
        "my_method",
        &StdJsonSerialize,
    )
    .await;

    assert_eq!(nats.published_messages(), vec!["_INBOX.reply"]);
}

#[tokio::test]
async fn request_missing_params_publishes_error_reply() {
    let nats = MockNatsClient::new();
    let client = MockClient::new();
    let envelope = Request::<Arc<RawValue>> {
        id: RequestId::Number(1),
        method: Arc::from("_my_method"),
        params: None,
    };
    let payload = serde_json::to_vec(&envelope).unwrap();

    handle(
        &payload,
        &client,
        Some("_INBOX.err"),
        &nats,
        "my_method",
        &StdJsonSerialize,
    )
    .await;

    assert_eq!(nats.published_messages(), vec!["_INBOX.err"]);
    let payloads = nats.published_payloads();
    let parsed: serde_json::Value = serde_json::from_slice(payloads[0].as_ref()).unwrap();
    assert_eq!(
        parsed.get("error").and_then(|e| e.get("code")),
        Some(&serde_json::Value::from(-32602))
    );
}

// --- notification tests ---

#[tokio::test]
async fn notification_forwards_to_client() {
    let nats = MockNatsClient::new();
    let client = MockClient::new();
    let payload = br#"{"event":"ping"}"#;

    handle(payload, &client, None, &nats, "my_notify", &StdJsonSerialize).await;

    assert_eq!(client.notification_count(), 1);
    assert!(nats.published_messages().is_empty());
}

#[tokio::test]
async fn notification_invalid_payload_does_not_panic() {
    let nats = MockNatsClient::new();
    let client = MockClient::new();

    handle(b"not json", &client, None, &nats, "my_notify", &StdJsonSerialize).await;

    assert_eq!(client.notification_count(), 0);
}

#[tokio::test]
async fn notification_client_error_does_not_panic() {
    let nats = MockNatsClient::new();
    let client = FailingClient;
    let payload = br#"{"event":"ping"}"#;

    handle(payload, &client, None, &nats, "my_notify", &StdJsonSerialize).await;
}

// --- error type tests ---

#[test]
fn error_code_and_message_malformed_json() {
    let err = ExtError::MalformedJson(serde_json::from_slice::<()>(b"bad").unwrap_err());
    let (code, msg) = error_code_and_message(&err);
    assert_eq!(code, ErrorCode::ParseError);
    assert!(msg.contains("Malformed ext request JSON"));
}

#[test]
fn error_code_and_message_missing_params() {
    let (code, msg) = error_code_and_message(&ExtError::MissingParams);
    assert_eq!(code, ErrorCode::InvalidParams);
    assert!(msg.contains("params is null or missing"));
}

#[test]
fn error_code_and_message_client_error() {
    let err = ExtError::ClientError(agent_client_protocol::Error::new(
        ErrorCode::InvalidParams.into(),
        "bad request",
    ));
    let (code, msg) = error_code_and_message(&err);
    assert_eq!(code, ErrorCode::InvalidParams);
    assert_eq!(msg, "bad request");
}

#[test]
fn ext_error_display() {
    let malformed = ExtError::MalformedJson(serde_json::from_slice::<()>(b"bad").unwrap_err());
    assert!(malformed.to_string().contains("malformed JSON"));

    let missing = ExtError::MissingParams;
    assert!(missing.to_string().contains("params is null or missing"));

    let client = ExtError::ClientError(agent_client_protocol::Error::new(-1, "fail"));
    assert!(client.to_string().contains("client error"));
}

#[test]
fn ext_error_source() {
    let malformed = ExtError::MalformedJson(serde_json::from_slice::<()>(b"bad").unwrap_err());
    assert!(malformed.source().is_some());

    let missing = ExtError::MissingParams;
    assert!(missing.source().is_none());

    let client = ExtError::ClientError(agent_client_protocol::Error::new(-1, "fail"));
    assert!(client.source().is_some());
}

#[tokio::test]
async fn forward_request_missing_params_returns_error() {
    let client = MockClient::new();
    let envelope = Request::<Arc<RawValue>> {
        id: RequestId::Number(1),
        method: Arc::from("_my_method"),
        params: None,
    };
    let payload = serde_json::to_vec(&envelope).unwrap();

    let result = forward_request(&payload, &client, "my_method").await;
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), ExtError::MissingParams));
}
