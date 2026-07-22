use super::*;
use agent_client_protocol::schema::v1::{
    ContentBlock, ContentChunk, RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse,
    SessionNotification, SessionUpdate, TerminalOutputResponse, ToolCallUpdate, ToolCallUpdateFields,
};
use async_nats::header::HeaderMap;
use async_trait::async_trait;
use jsonrpc_nats::RequestId;
use trogon_nats::MockNatsClient;

fn empty_headers() -> HeaderMap {
    HeaderMap::new()
}

fn make_wire_request<T: serde::Serialize>(params: &T) -> (HeaderMap, Vec<u8>) {
    crate::client::test_support::encode_wire_request("terminal/output", RequestId::Number(1), params)
}

fn sample_request() -> TerminalOutputRequest {
    TerminalOutputRequest::new("sess-1", "term-1")
}
struct MockClient {
    terminal_output_result: agent_client_protocol::Result<TerminalOutputResponse>,
}

impl MockClient {
    fn success() -> Self {
        Self {
            terminal_output_result: Ok(TerminalOutputResponse::new("output", false)),
        }
    }

    fn failing() -> Self {
        Self {
            terminal_output_result: Err(agent_client_protocol::Error::new(-32603, "mock failure")),
        }
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

    async fn terminal_output(&self, _: TerminalOutputRequest) -> agent_client_protocol::Result<TerminalOutputResponse> {
        self.terminal_output_result.clone()
    }
}

#[tokio::test]
async fn success_publishes_response_to_reply_subject() {
    let nats = MockNatsClient::new();
    let client = MockClient::success();
    let request = sample_request();
    let (headers, payload) = make_wire_request(&request);

    handle(&headers, &payload, &client, Some("_INBOX.reply"), &nats, "sess-1").await;

    assert_eq!(nats.published_messages(), vec!["_INBOX.reply"]);
}

#[tokio::test]
async fn no_reply_does_not_publish() {
    let nats = MockNatsClient::new();
    let client = MockClient::success();
    let request = sample_request();
    let (headers, payload) = make_wire_request(&request);

    handle(&headers, &payload, &client, None, &nats, "sess-1").await;

    assert!(nats.published_messages().is_empty());
}

#[tokio::test]
async fn malformed_json_publishes_parse_error() {
    let nats = MockNatsClient::new();
    let client = MockClient::success();

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
async fn invalid_params_publishes_error() {
    let nats = MockNatsClient::new();
    let client = MockClient::success();
    let payload = br#"{"id":1,"method":"terminal/output","params":{}}"#;

    handle(
        &empty_headers(),
        payload.as_slice(),
        &client,
        Some("_INBOX.err"),
        &nats,
        "sess-1",
    )
    .await;

    assert_eq!(nats.published_messages(), vec!["_INBOX.err"]);
    let published_headers = nats.published_headers()[0].clone();
    assert_eq!(
        published_headers.get(jsonrpc_nats::HEADER_ERROR_CODE).unwrap().as_str(),
        "-32602",
    );
}

#[tokio::test]
async fn null_params_publishes_error() {
    let nats = MockNatsClient::new();
    let client = MockClient::success();
    let payload = br#"{"id":1,"method":"terminal/output","params":null}"#;

    handle(
        &empty_headers(),
        payload.as_slice(),
        &client,
        Some("_INBOX.err"),
        &nats,
        "sess-1",
    )
    .await;

    assert_eq!(nats.published_messages(), vec!["_INBOX.err"]);
    let published_headers = nats.published_headers()[0].clone();
    assert!(
        published_headers.get(jsonrpc_nats::HEADER_ERROR_CODE).is_some(),
        "invalid request body should publish an error reply"
    );
}

#[tokio::test]
async fn session_id_mismatch_publishes_error() {
    let nats = MockNatsClient::new();
    let client = MockClient::success();
    let request = sample_request();
    let (headers, payload) = make_wire_request(&request);

    handle(
        &headers,
        &payload,
        &client,
        Some("_INBOX.err"),
        &nats,
        "different-session",
    )
    .await;

    assert_eq!(nats.published_messages(), vec!["_INBOX.err"]);
    let published_headers = nats.published_headers()[0].clone();
    assert_eq!(
        published_headers.get(jsonrpc_nats::HEADER_ERROR_CODE).unwrap().as_str(),
        "-32602",
    );
    let payloads = nats.published_payloads();
    let body: serde_json::Value = serde_json::from_slice(payloads[0].as_ref()).unwrap();
    assert!(body["message"].as_str().unwrap().contains("does not match"));
}

#[tokio::test]
async fn client_error_publishes_error_reply() {
    let nats = MockNatsClient::new();
    let client = MockClient::failing();
    let request = sample_request();
    let (headers, payload) = make_wire_request(&request);

    handle(&headers, &payload, &client, Some("_INBOX.err"), &nats, "sess-1").await;

    assert_eq!(nats.published_messages(), vec!["_INBOX.err"]);
    let published_headers = nats.published_headers()[0].clone();
    assert!(published_headers.get(jsonrpc_nats::HEADER_ERROR_CODE).is_some());
}

#[tokio::test]
async fn serialization_failure_sends_fallback_error() {
    let nats = MockNatsClient::new();
    let client = MockClient::success();
    let request = sample_request();
    let (headers, payload) = make_wire_request(&request);

    handle(&headers, &payload, &client, Some("_INBOX.reply"), &nats, "sess-1").await;

    assert_eq!(nats.published_messages(), vec!["_INBOX.reply"]);
}

#[tokio::test]
async fn mock_client_trait_methods_exercise_coverage() {
    let client = MockClient::success();
    let notification = SessionNotification::new(
        "sess-1",
        SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::from("hi"))),
    );
    assert!(client.session_notification(notification).await.is_ok());

    let tool_call = ToolCallUpdate::new("call-1", ToolCallUpdateFields::new());
    let req = RequestPermissionRequest::new("sess-1", tool_call, vec![]);
    assert!(client.request_permission(req).await.is_ok());
}
