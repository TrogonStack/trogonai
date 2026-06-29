use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use rmcp::model::{
    ClientCapabilities, ClientInfo, ClientRequest, Implementation, InitializeRequest, InitializeRequestParams,
    InitializeResult, JsonRpcMessage, NumberOrString, ServerCapabilities, ServerResult,
};
use rmcp::service::RoleServer;
use tower::ServiceExt;

use mcp_nats::wire;

use super::*;

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
    nats.set_response_wire("mcp.server.default.initialize", encoded.headers, encoded.body);
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
    assert!(nats.subscribed_to()[0].starts_with("mcp.client.http-"));
    assert!(nats.subscribed_to()[0].ends_with(".>"));
    let response_body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body = String::from_utf8(response_body.to_vec()).unwrap();
    assert!(body.contains("remote-server"));
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
    nats.set_response_wire("mcp.server.default.initialize", encoded.headers, encoded.body);
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
