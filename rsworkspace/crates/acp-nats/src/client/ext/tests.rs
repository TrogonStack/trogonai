use super::*;
use agent_client_protocol::schema::v1::{
    ExtRequest, ExtResponse, RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse,
    SessionNotification,
};
use async_nats::header::HeaderMap;
use async_trait::async_trait;
use jsonrpc_nats::RequestId;
use std::error::Error;
use std::sync::{Arc, Mutex};
use trogon_nats::{AdvancedMockNatsClient, MockNatsClient};

struct MockClient {
    notifications: Mutex<Vec<String>>,
}

impl MockClient {
    fn new() -> Self {
        Self {
            notifications: Mutex::new(Vec::new()),
        }
    }

    fn notification_count(&self) -> usize {
        self.notifications.lock().unwrap().len()
    }
}

#[async_trait]
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

    async fn ext_method(&self, _: ExtRequest) -> agent_client_protocol::Result<ExtResponse> {
        let raw = RawValue::from_string(r#"{"status":"ok"}"#.to_string()).unwrap();
        Ok(ExtResponse::new(raw.into()))
    }

    async fn ext_notification(&self, args: ExtNotification) -> agent_client_protocol::Result<()> {
        self.notifications.lock().unwrap().push(args.method.to_string());
        Ok(())
    }
}

struct FailingClient;

#[async_trait]
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

fn empty_headers() -> HeaderMap {
    HeaderMap::new()
}

fn make_ext_wire_request(method: &str, params_json: &str) -> (HeaderMap, Vec<u8>) {
    let raw = RawValue::from_string(params_json.to_string()).unwrap();
    crate::client::test_support::encode_wire_request(
        &format!("_{method}"),
        RequestId::Number(1),
        &Arc::<RawValue>::from(raw),
    )
}

fn make_ext_wire_notification(method: &str, params_json: &str) -> (HeaderMap, Vec<u8>) {
    let value: serde_json::Value = serde_json::from_str(params_json).unwrap();
    crate::client::test_support::encode_wire_notification(&format!("_{method}"), &value)
}

// --- request/response tests ---

#[tokio::test]
async fn request_publishes_response_to_reply_subject() {
    let nats = MockNatsClient::new();
    let client = MockClient::new();
    let (headers, payload) = make_ext_wire_request("my_method", r#"{"key":"value"}"#);

    handle(&headers, &payload, &client, Some("_INBOX.reply"), &nats, "my_method").await;

    assert_eq!(nats.published_messages(), vec!["_INBOX.reply"]);
    let payloads = nats.published_payloads();
    let parsed: serde_json::Value = serde_json::from_slice(payloads[0].as_ref()).unwrap();
    assert!(parsed.get("status").is_some());
}

#[tokio::test]
async fn request_invalid_payload_publishes_error_reply() {
    let nats = MockNatsClient::new();
    let client = MockClient::new();

    handle(
        &empty_headers(),
        b"not json",
        &client,
        Some("_INBOX.err"),
        &nats,
        "my_method",
    )
    .await;

    assert_eq!(nats.published_messages(), vec!["_INBOX.err"]);
    let headers = nats.published_headers()[0].clone();
    assert_eq!(headers.get(jsonrpc_nats::HEADER_ERROR_CODE).unwrap().as_str(), "-32700");
}

#[tokio::test]
async fn request_client_error_publishes_error_reply() {
    let nats = MockNatsClient::new();
    let client = FailingClient;
    let (headers, payload) = make_ext_wire_request("my_method", r#"{}"#);

    handle(&headers, &payload, &client, Some("_INBOX.err"), &nats, "my_method").await;

    assert_eq!(nats.published_messages(), vec!["_INBOX.err"]);
    let published_headers = nats.published_headers()[0].clone();
    assert!(published_headers.get(jsonrpc_nats::HEADER_ERROR_CODE).is_some());
    let payloads = nats.published_payloads();
    let body: serde_json::Value = serde_json::from_slice(payloads[0].as_ref()).unwrap();
    assert_eq!(body["message"], "ext method failed");
}

#[tokio::test]
async fn request_publish_failure_exercises_error_path() {
    let nats = AdvancedMockNatsClient::new();
    nats.fail_next_publish();
    let client = MockClient::new();
    let (headers, payload) = make_ext_wire_request("my_method", r#"{}"#);

    handle(&headers, &payload, &client, Some("_INBOX.reply"), &nats, "my_method").await;

    assert!(nats.published_messages().is_empty());
}

#[tokio::test]
async fn request_flush_failure_exercises_warn_path() {
    let nats = AdvancedMockNatsClient::new();
    nats.fail_next_flush();
    let client = MockClient::new();
    let (headers, payload) = make_ext_wire_request("my_method", r#"{}"#);

    handle(&headers, &payload, &client, Some("_INBOX.reply"), &nats, "my_method").await;

    assert_eq!(nats.published_messages(), vec!["_INBOX.reply"]);
}

#[tokio::test]
async fn request_missing_params_publishes_error_reply() {
    let nats = MockNatsClient::new();
    let client = MockClient::new();

    handle(&empty_headers(), b"{}", &client, Some("_INBOX.err"), &nats, "my_method").await;

    assert_eq!(nats.published_messages(), vec!["_INBOX.err"]);
    let headers = nats.published_headers()[0].clone();
    assert_eq!(headers.get(jsonrpc_nats::HEADER_ERROR_CODE).unwrap().as_str(), "-32700");
}

// --- notification tests ---

#[tokio::test]
async fn notification_forwards_to_client() {
    let nats = MockNatsClient::new();
    let client = MockClient::new();
    let (headers, payload) = make_ext_wire_notification("my_notify", r#"{"event":"ping"}"#);

    handle(&headers, &payload, &client, None, &nats, "my_notify").await;

    assert_eq!(client.notification_count(), 1);
    assert!(nats.published_messages().is_empty());
}

#[tokio::test]
async fn notification_invalid_payload_does_not_panic() {
    let nats = MockNatsClient::new();
    let client = MockClient::new();

    handle(&empty_headers(), b"not json", &client, None, &nats, "my_notify").await;

    assert_eq!(client.notification_count(), 0);
}

#[tokio::test]
async fn notification_client_error_does_not_panic() {
    let nats = MockNatsClient::new();
    let client = FailingClient;
    let (headers, payload) = make_ext_wire_notification("my_notify", r#"{"event":"ping"}"#);

    handle(&headers, &payload, &client, None, &nats, "my_notify").await;
}

// --- error type tests ---

#[test]
fn error_code_and_message_malformed_json() {
    let err = ExtError::MalformedJson("bad json".to_string());
    let (code, msg) = error_code_and_message(&err);
    assert_eq!(code, ErrorCode::ParseError);
    assert!(msg.contains("Malformed ext request JSON"));
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
    let malformed = ExtError::MalformedJson("bad".to_string());
    assert_eq!(malformed.to_string(), "malformed JSON: bad");

    let client = ExtError::ClientError(agent_client_protocol::Error::new(-1, "fail"));
    assert_eq!(client.to_string(), "client error: fail");
}

#[test]
fn ext_error_source() {
    let malformed = ExtError::MalformedJson("bad".to_string());
    assert!(malformed.source().is_none());

    let client = ExtError::ClientError(agent_client_protocol::Error::new(-1, "fail"));
    assert!(client.source().is_some());
}

#[tokio::test]
async fn forward_request_invalid_payload_returns_error() {
    let client = MockClient::new();

    let result = forward_request(&empty_headers(), b"{}", &client, "my_method", "_my_method").await;
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), ExtError::MalformedJson(_)));
}
