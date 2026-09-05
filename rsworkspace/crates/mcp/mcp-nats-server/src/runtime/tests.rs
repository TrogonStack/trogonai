use std::sync::{Arc, Mutex};

use async_nats::{HeaderMap, Message};
use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use rmcp::model::{
    ClientCapabilities, ClientInfo, ClientRequest, Implementation, InitializeRequest, InitializeRequestParams,
    InitializeResult, JsonRpcMessage, NumberOrString, ServerCapabilities, ServerResult,
};
use rmcp::service::RoleServer;
use tower::ServiceExt;
use trogon_nats::ToSubject;

use mcp_nats::wire;

use super::*;

mod forwarding_tests;
mod protocol_surface_tests;
mod worker_lifecycle_tests;

#[derive(Clone, Debug)]
struct CapturedNatsRequest {
    subject: String,
    headers: HeaderMap,
    payload: axum::body::Bytes,
}

#[derive(Clone, Debug)]
struct CapturingNatsClient {
    inner: trogon_nats::AdvancedMockNatsClient,
    requests: Arc<Mutex<Vec<CapturedNatsRequest>>>,
}

impl CapturingNatsClient {
    fn new() -> Self {
        Self {
            inner: trogon_nats::AdvancedMockNatsClient::new(),
            requests: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn set_response_wire(&self, subject: &str, headers: HeaderMap, payload: axum::body::Bytes) {
        self.inner.set_response_wire(subject, headers, payload);
    }

    fn captured_requests(&self) -> Vec<CapturedNatsRequest> {
        self.requests.lock().unwrap().clone()
    }

    fn clear_captured_requests(&self) {
        self.requests.lock().unwrap().clear();
    }
}

impl SubscribeClient for CapturingNatsClient {
    type SubscribeError = <trogon_nats::AdvancedMockNatsClient as SubscribeClient>::SubscribeError;
    type Subscription = <trogon_nats::AdvancedMockNatsClient as SubscribeClient>::Subscription;

    async fn subscribe<S: ToSubject + Send>(&self, subject: S) -> Result<Self::Subscription, Self::SubscribeError> {
        self.inner.subscribe(subject).await
    }
}

impl RequestClient for CapturingNatsClient {
    type RequestError = <trogon_nats::AdvancedMockNatsClient as RequestClient>::RequestError;

    async fn request_with_headers<S: ToSubject + Send>(
        &self,
        subject: S,
        headers: HeaderMap,
        payload: axum::body::Bytes,
    ) -> Result<Message, Self::RequestError> {
        let subject = subject.to_subject().to_string();
        self.requests.lock().unwrap().push(CapturedNatsRequest {
            subject: subject.clone(),
            headers: headers.clone(),
            payload: payload.clone(),
        });
        self.inner.request_with_headers(subject, headers, payload).await
    }
}

impl PublishClient for CapturingNatsClient {
    type PublishError = <trogon_nats::AdvancedMockNatsClient as PublishClient>::PublishError;

    async fn publish_with_headers<S: ToSubject + Send>(
        &self,
        subject: S,
        headers: HeaderMap,
        payload: axum::body::Bytes,
    ) -> Result<(), Self::PublishError> {
        self.inner.publish_with_headers(subject, headers, payload).await
    }
}

impl FlushClient for CapturingNatsClient {
    type FlushError = <trogon_nats::AdvancedMockNatsClient as FlushClient>::FlushError;

    async fn flush(&self) -> Result<(), Self::FlushError> {
        self.inner.flush().await
    }
}

fn mcp_config() -> Config {
    Config::new(
        mcp_nats::McpPrefix::new("mcp").unwrap(),
        trogon_nats::NatsConfig {
            servers: vec!["localhost:4222".to_string()],
            auth: trogon_nats::NatsAuth::None,
        },
    )
}

fn initialize_request() -> ClientJsonRpcMessage {
    ClientJsonRpcMessage::request(
        ClientRequest::InitializeRequest(InitializeRequest::new(InitializeRequestParams::new(
            ClientCapabilities::default(),
            Implementation::new("test-client", "1.0.0"),
        ))),
        NumberOrString::Number(1),
    )
}

fn initialize_response() -> ServerJsonRpcMessage {
    ServerJsonRpcMessage::response(
        ServerResult::InitializeResult(
            InitializeResult::new(ServerCapabilities::default())
                .with_server_info(Implementation::new("remote-server", "1.0.0")),
        ),
        NumberOrString::Number(1),
    )
}

#[tokio::test]
async fn streamable_http_service_routes_initialize_to_nats_server() {
    let nats = trogon_nats::AdvancedMockNatsClient::new();
    let _inbound = nats.inject_messages();
    let encoded = wire::encode_tx::<RoleServer>(&initialize_response()).unwrap();
    nats.set_response_wire("mcp.v1.server.default.initialize", encoded.headers, encoded.body);
    let service = streamable_http_service(
        nats.clone(),
        mcp_config(),
        ClientIdFactory::new(McpPeerId::new("http").unwrap()),
        McpPeerId::new("default").unwrap(),
        StreamableHttpServerConfig::default(),
    );
    let app = Router::new().route_service("/mcp", service);
    let body = serde_json::to_vec(&initialize_request()).unwrap();

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/mcp")
                .header(header::HOST, "localhost")
                .header(header::ACCEPT, "application/json, text/event-stream")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.headers().contains_key("mcp-session-id"));
    assert_eq!(nats.subscribed_to().len(), 1);
    assert!(nats.subscribed_to()[0].starts_with("mcp.v1.client.http-"));
    assert!(nats.subscribed_to()[0].ends_with(".>"));
    let response_body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body = String::from_utf8(response_body.to_vec()).unwrap();
    assert!(body.contains("remote-server"));
}

#[tokio::test]
async fn streamable_http_preserves_request_meta_and_allowlisted_headers_on_nats() {
    let nats = CapturingNatsClient::new();
    let _inbound = nats.inner.inject_messages();
    let initialize = wire::encode_tx::<RoleServer>(&initialize_response()).unwrap();
    nats.set_response_wire("mcp.v1.server.default.initialize", initialize.headers, initialize.body);
    let tool_error = ServerJsonRpcMessage::error(
        ErrorData::internal_error("expected test response", None),
        Some(NumberOrString::Number(2)),
    );
    let tool_error = wire::encode_tx::<RoleServer>(&tool_error).unwrap();
    nats.set_response_wire("mcp.v1.server.default.tools.call", tool_error.headers, tool_error.body);
    let service = streamable_http_service(
        nats.clone(),
        mcp_config(),
        ClientIdFactory::new(McpPeerId::new("http").unwrap()),
        McpPeerId::new("default").unwrap(),
        StreamableHttpServerConfig::default(),
    );
    let app = Router::new().route_service("/mcp", service);

    let initialize_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/mcp")
                .header(header::HOST, "localhost")
                .header(header::ACCEPT, "application/json, text/event-stream")
                .header(header::CONTENT_TYPE, "application/json")
                .header("MCP-Protocol-Version", "2025-11-25")
                .header("Mcp-Method", "initialize")
                .body(Body::from(serde_json::to_vec(&initialize_request()).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(initialize_response.status(), StatusCode::OK);
    let session_id = initialize_response.headers().get("mcp-session-id").unwrap().clone();

    let initialized = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized",
        "params": {
            "_meta": {"test.notification": "preserved"}
        }
    });
    let notification_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/mcp")
                .header(header::HOST, "localhost")
                .header(header::ACCEPT, "application/json, text/event-stream")
                .header(header::CONTENT_TYPE, "application/json")
                .header("Mcp-Session-Id", session_id)
                .header("MCP-Protocol-Version", "2025-11-25")
                .header("Mcp-Method", "notifications/initialized")
                .header("Authorization", "Bearer must-not-cross")
                .header("Cookie", "session=must-not-cross")
                .body(Body::from(serde_json::to_vec(&initialized).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(notification_response.status().is_success());
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        while nats.inner.published_headers().is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let published_headers = nats.inner.published_headers();
    assert_eq!(published_headers.len(), 1);
    assert_eq!(
        published_headers[0]
            .get("MCP-Protocol-Version")
            .map(|value| value.as_str()),
        Some("2025-11-25")
    );
    assert_eq!(
        published_headers[0].get("Mcp-Method").map(|value| value.as_str()),
        Some("notifications/initialized")
    );
    assert!(published_headers[0].get("Authorization").is_none());
    assert!(published_headers[0].get("Cookie").is_none());
    assert!(published_headers[0].get("Mcp-Session-Id").is_none());
    let published_payloads = nats.inner.published_payloads();
    let notification_body: serde_json::Value = serde_json::from_slice(&published_payloads[0]).unwrap();
    assert_eq!(notification_body["params"]["_meta"]["test.notification"], "preserved");
    nats.clear_captured_requests();

    let _schema_inbound = nats.inner.inject_messages();
    let _request_inbound = nats.inner.inject_messages();
    let call_tool = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "_meta": {
                "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                "io.modelcontextprotocol/clientCapabilities": {},
                "test.marker": "preserved"
            },
            "name": "deploy",
            "arguments": {"region": "us-west1"}
        }
    });
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/mcp")
                .header(header::HOST, "localhost")
                .header(header::ACCEPT, "application/json, text/event-stream")
                .header(header::CONTENT_TYPE, "application/json")
                .header("MCP-Protocol-Version", "2026-07-28")
                .header("Mcp-Method", "tools/call")
                .header("Mcp-Name", "deploy")
                .header("Mcp-Param-Region", "us-west1")
                .header("Authorization", "Bearer must-not-cross")
                .body(Body::from(serde_json::to_vec(&call_tool).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let response_body = to_bytes(response.into_body(), usize::MAX).await.unwrap();

    let captured = nats.captured_requests();
    assert_eq!(
        captured.len(),
        1,
        "unexpected HTTP response: {}",
        String::from_utf8_lossy(&response_body)
    );
    let captured = &captured[0];
    assert_eq!(captured.subject, "mcp.v1.server.default.tools.call");
    assert_eq!(
        captured.headers.get("MCP-Protocol-Version").map(|value| value.as_str()),
        Some("2026-07-28")
    );
    assert_eq!(
        captured.headers.get("Mcp-Method").map(|value| value.as_str()),
        Some("tools/call")
    );
    assert_eq!(
        captured.headers.get("Mcp-Name").map(|value| value.as_str()),
        Some("deploy")
    );
    assert_eq!(
        captured.headers.get("Mcp-Param-region").map(|value| value.as_str()),
        Some("us-west1")
    );
    assert!(captured.headers.get("Authorization").is_none());
    assert!(captured.headers.get("Mcp-Session-Id").is_none());
    let body: serde_json::Value = serde_json::from_slice(&captured.payload).unwrap();
    assert_eq!(body["jsonrpc"], "2.0");
    assert_eq!(body["id"], 2);
    assert_eq!(body["method"], "tools/call");
    assert_eq!(body["params"]["_meta"]["test.marker"], "preserved");
    assert_eq!(
        body["params"]["_meta"]["io.modelcontextprotocol/protocolVersion"],
        "2026-07-28"
    );
}

#[tokio::test]
async fn request_fails_when_nats_response_id_never_matches() {
    let nats = trogon_nats::AdvancedMockNatsClient::new();
    let _inbound = nats.inject_messages();
    let mismatched = ServerJsonRpcMessage::response(
        ServerResult::InitializeResult(
            InitializeResult::new(ServerCapabilities::default())
                .with_server_info(Implementation::new("remote-server", "1.0.0")),
        ),
        NumberOrString::Number(99),
    );
    let encoded = wire::encode_tx::<RoleServer>(&mismatched).unwrap();
    nats.set_response_wire("mcp.v1.server.default.initialize", encoded.headers, encoded.body);
    let service = streamable_http_service(
        nats.clone(),
        mcp_config().with_operation_timeout(std::time::Duration::from_secs(1)),
        ClientIdFactory::new(McpPeerId::new("http").unwrap()),
        McpPeerId::new("default").unwrap(),
        StreamableHttpServerConfig::default(),
    );
    let app = Router::new().route_service("/mcp", service);
    let body = serde_json::to_vec(&initialize_request()).unwrap();

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/mcp")
                .header(header::HOST, "localhost")
                .header(header::ACCEPT, "application/json, text/event-stream")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();

    let response_body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body = String::from_utf8(response_body.to_vec()).unwrap();
    assert!(body.contains("timed out"));
}

#[tokio::test]
async fn handle_remote_message_delivers_error_to_pending_request() {
    let nats = trogon_nats::AdvancedMockNatsClient::new();
    let _inbound = nats.inject_messages();
    let mut transport = mcp_nats::client::connect(
        nats,
        &mcp_config(),
        McpPeerId::new("http-test").unwrap(),
        McpPeerId::new("default").unwrap(),
    )
    .await
    .unwrap();

    let (response_tx, response_rx) = oneshot::channel();
    let mut pending: HashMap<RequestId, PendingEntry> = HashMap::new();
    pending.insert(
        RequestId::Number(7),
        PendingEntry {
            response_tx,
            deadline: Instant::now(),
        },
    );

    let message = ServerJsonRpcMessage::error(
        ErrorData::internal_error("remote failure", None),
        Some(RequestId::Number(7)),
    );

    handle_remote_message(message, &mut transport, None, &mut pending).await;

    assert!(pending.is_empty());
    let delivered = response_rx.await.unwrap();
    assert_eq!(delivered.unwrap_err().message.as_ref(), "remote failure");
}

#[test]
fn client_id_factory_generates_valid_unique_peer_ids() {
    let factory = ClientIdFactory::new(McpPeerId::new("http").unwrap());

    let first = factory.next().unwrap();
    let second = factory.next().unwrap();

    assert!(first.as_str().starts_with("http-"));
    assert!(second.as_str().starts_with("http-"));
    assert_ne!(first, second);
}

#[test]
fn streamable_http_config_uses_sdk_defaults_unless_allowed_hosts_are_overridden() {
    assert_eq!(
        streamable_http_config(Vec::new()).allowed_hosts,
        StreamableHttpServerConfig::default().allowed_hosts
    );
    assert_eq!(
        streamable_http_config(vec![AllowedHost::new("example.com").unwrap()]).allowed_hosts,
        vec!["example.com"]
    );
}

#[tokio::test]
async fn service_info_is_available_before_remote_initialize() {
    let nats = trogon_nats::AdvancedMockNatsClient::new();
    let service = McpNatsProxyService::new(
        nats,
        mcp_config(),
        McpPeerId::new("http-test").unwrap(),
        McpPeerId::new("default").unwrap(),
    );

    let info: ServerInfo = service.get_info();

    assert!(!info.server_info.name.is_empty());
}

#[tokio::test]
async fn remembered_server_info_replaces_the_placeholder() {
    let nats = trogon_nats::AdvancedMockNatsClient::new();
    let service = McpNatsProxyService::new(
        nats,
        mcp_config(),
        McpPeerId::new("http-test").unwrap(),
        McpPeerId::new("default").unwrap(),
    );
    let remote = InitializeResult::new(ServerCapabilities::default())
        .with_server_info(Implementation::new("remote-server", "9.9.9"));

    service.remember_server_info(&remote);

    let info: ServerInfo = service.get_info();
    assert_eq!(info.server_info.name, "remote-server");
    assert_eq!(info.server_info.version, "9.9.9");
}

#[test]
fn initialize_request_uses_client_info_type() {
    let message = initialize_request();

    let JsonRpcMessage::Request(request) = message else {
        panic!("expected initialize request");
    };
    let ClientRequest::InitializeRequest(InitializeRequest { params, .. }) = request.request else {
        panic!("expected initialize method");
    };
    let _: ClientInfo = params;
}

#[test]
fn empty_context_meta_does_not_fabricate_no_param_payloads() {
    let mut request_extensions = Extensions::new();
    request_extensions.insert(RequestMetaObject::new());
    restore_request_meta(&mut request_extensions, RequestMetaObject::new());
    let request = rmcp::model::PingRequest {
        extensions: request_extensions,
        ..Default::default()
    };

    let mut notification_extensions = Extensions::new();
    notification_extensions.insert(NotificationMetaObject::new());
    restore_notification_meta(&mut notification_extensions, NotificationMetaObject::new());
    let notification = rmcp::model::InitializedNotification {
        extensions: notification_extensions,
        ..Default::default()
    };

    let request = serde_json::to_value(request).unwrap();
    let notification = serde_json::to_value(notification).unwrap();
    assert!(request.get("params").is_none());
    assert!(notification.get("params").is_none());
}

#[test]
fn custom_request_extensions_restore_meta_and_allowlisted_http_headers() {
    let (parts, ()) = Request::builder()
        .header("MCP-Protocol-Version", "2026-07-28")
        .header("Mcp-Method", "example/custom")
        .header("Mcp-Session-Id", "private-session")
        .header("Authorization", "Bearer private")
        .body(())
        .unwrap()
        .into_parts();
    let mut extensions = Extensions::new();
    extensions.insert(parts);
    let mut meta = RequestMetaObject::new();
    meta.insert("test.marker".to_owned(), serde_json::json!("preserved"));

    restore_request_meta(&mut extensions, meta);
    preserve_http_transport_headers(&mut extensions);
    let mut request = rmcp::model::CustomRequest::new("example/custom", None);
    request.extensions = extensions;

    assert_eq!(
        request
            .extensions
            .get::<RequestMetaObject>()
            .and_then(|meta| meta.get("test.marker")),
        Some(&serde_json::json!("preserved"))
    );
    let headers = request.extensions.get::<McpTransportHeaders>().unwrap();
    assert_eq!(headers.get("MCP-Protocol-Version"), Some("2026-07-28"));
    assert_eq!(headers.get("Mcp-Method"), Some("example/custom"));
    assert_eq!(headers.get("Mcp-Session-Id"), None);
    assert_eq!(headers.get("Authorization"), None);
    let request = serde_json::to_value(request).unwrap();
    assert_eq!(request["params"]["_meta"]["test.marker"], "preserved");
}

#[derive(Clone)]
struct NoopServerHandler;

impl ServerHandler for NoopServerHandler {}

#[tokio::test]
async fn proxy_worker_times_out_pending_requests_that_never_get_a_response() {
    let nats = trogon_nats::AdvancedMockNatsClient::new();
    let _inbound = nats.inject_messages();
    let mismatched = ServerJsonRpcMessage::response(
        ServerResult::InitializeResult(
            InitializeResult::new(ServerCapabilities::default())
                .with_server_info(Implementation::new("remote-server", "1.0.0")),
        ),
        NumberOrString::Number(99),
    );
    let encoded = wire::encode_tx::<RoleServer>(&mismatched).unwrap();
    nats.set_response_wire("mcp.v1.server.default.initialize", encoded.headers, encoded.body);

    let (_http_side, handler_side) = tokio::io::duplex(1024);
    let running = rmcp::service::serve_directly(NoopServerHandler, handler_side, None);
    let peer = running.peer().clone();

    let (command_tx, command_rx) = mpsc::channel(1);
    let worker = tokio::spawn(run_proxy_worker(
        nats,
        mcp_config().with_operation_timeout(Duration::from_millis(150)),
        McpPeerId::new("http-test").unwrap(),
        McpPeerId::new("default").unwrap(),
        command_rx,
    ));

    let JsonRpcMessage::Request(request) = initialize_request() else {
        panic!("expected initialize request");
    };
    let (response_tx, response_rx) = oneshot::channel();
    command_tx
        .send(ProxyCommand::Request {
            request: Box::new(request.request),
            request_id: RequestId::Number(1),
            peer,
            response_tx,
        })
        .await
        .unwrap();

    let delivered = response_rx.await.unwrap();
    assert_eq!(
        delivered.unwrap_err().message.as_ref(),
        "MCP NATS proxy timed out waiting for a response"
    );

    drop(command_tx);
    worker.await.unwrap();
}

#[tokio::test]
async fn evict_expired_pending_keeps_requests_that_are_still_within_their_deadline() {
    let (expired_tx, expired_rx) = oneshot::channel();
    let (live_tx, _live_rx) = oneshot::channel();
    let mut pending: HashMap<RequestId, PendingEntry> = HashMap::new();
    pending.insert(
        RequestId::Number(1),
        PendingEntry {
            response_tx: expired_tx,
            deadline: Instant::now(),
        },
    );
    pending.insert(
        RequestId::Number(2),
        PendingEntry {
            response_tx: live_tx,
            deadline: Instant::now() + Duration::from_secs(60),
        },
    );

    evict_expired_pending(&mut pending);

    assert_eq!(pending.len(), 1);
    assert!(pending.contains_key(&RequestId::Number(2)));
    assert_eq!(
        expired_rx.await.unwrap().unwrap_err().message.as_ref(),
        "MCP NATS proxy timed out waiting for a response"
    );
}

#[tokio::test]
async fn wait_for_deadline_never_resolves_without_a_pending_deadline() {
    assert!(
        tokio::time::timeout(Duration::from_millis(20), wait_for_deadline(None))
            .await
            .is_err()
    );
}
