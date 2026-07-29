use jsonrpc_nats::Direction;
use rmcp::model::{
    CallToolRequestParams, ClientNotification, ClientRequest, GetExtensions, InitializedNotification, JsonRpcMessage,
    Request, RequestId,
};
use rmcp::service::{RoleClient, RoleServer};
use rmcp::transport::common::http_header::{HEADER_MCP_METHOD, HEADER_MCP_NAME, HEADER_MCP_PROTOCOL_VERSION};

use crate::{ClientJsonRpcMessage, McpTransportHeaders};

use super::{decode_rx, encode_tx};

#[test]
fn decode_exposes_only_allowlisted_mcp_headers_as_typed_extensions() {
    let message = ClientJsonRpcMessage::request(
        ClientRequest::CallToolRequest(Request::new(CallToolRequestParams::new("deploy"))),
        RequestId::Number(7),
    );
    let mut encoded = encode_tx::<RoleClient>(&message).unwrap();
    encoded.headers.append(HEADER_MCP_PROTOCOL_VERSION, "2026-07-28");
    encoded.headers.append(HEADER_MCP_METHOD, "tools/call");
    encoded.headers.append(HEADER_MCP_NAME, "deploy");
    encoded.headers.append("mcp-param-region", "us-west1");
    encoded.headers.append("Authorization", "Bearer secret");

    let decoded =
        decode_rx::<RoleServer>(Direction::Request, Some("tools/call"), &encoded.headers, &encoded.body).unwrap();
    let JsonRpcMessage::Request(request) = decoded else {
        panic!("expected request");
    };
    let headers = request.request.extensions().get::<McpTransportHeaders>().unwrap();

    assert_eq!(headers.get(HEADER_MCP_PROTOCOL_VERSION), Some("2026-07-28"));
    assert_eq!(headers.get(HEADER_MCP_METHOD), Some("tools/call"));
    assert_eq!(headers.get(HEADER_MCP_NAME), Some("deploy"));
    assert_eq!(headers.get("Mcp-Param-Region"), Some("us-west1"));
    assert_eq!(headers.get("Authorization"), None);
}

#[test]
fn encode_carries_typed_headers_into_nats_headers() {
    let mut http_headers = http::HeaderMap::new();
    http_headers.insert(HEADER_MCP_PROTOCOL_VERSION, "2026-07-28".parse().unwrap());
    http_headers.insert(HEADER_MCP_METHOD, "tools/call".parse().unwrap());
    http_headers.insert(HEADER_MCP_NAME, "deploy".parse().unwrap());
    http_headers.insert("Mcp-Param-Region", "us-west1".parse().unwrap());
    http_headers.insert("Mcp-Session-Id", "private-session".parse().unwrap());
    http_headers.insert("Cookie", "session=secret".parse().unwrap());
    let mut request = Request::new(CallToolRequestParams::new("deploy"));
    request.extensions.insert(McpTransportHeaders::from_http(&http_headers));
    let message = ClientJsonRpcMessage::request(ClientRequest::CallToolRequest(request), RequestId::Number(8));

    let encoded = encode_tx::<RoleClient>(&message).unwrap();

    assert_eq!(
        encoded
            .headers
            .get(HEADER_MCP_PROTOCOL_VERSION)
            .map(|value| value.as_str()),
        Some("2026-07-28")
    );
    assert_eq!(
        encoded.headers.get("Mcp-Param-region").map(|value| value.as_str()),
        Some("us-west1")
    );
    assert!(encoded.headers.get("Mcp-Session-Id").is_none());
    assert!(encoded.headers.get("Cookie").is_none());
}

#[test]
fn notification_headers_round_trip_as_typed_extensions() {
    let mut http_headers = http::HeaderMap::new();
    http_headers.insert(HEADER_MCP_PROTOCOL_VERSION, "2026-07-28".parse().unwrap());
    http_headers.insert(HEADER_MCP_METHOD, "notifications/initialized".parse().unwrap());
    http_headers.insert("Authorization", "Bearer secret".parse().unwrap());
    let mut notification = InitializedNotification::default();
    notification
        .extensions
        .insert(McpTransportHeaders::from_http(&http_headers));
    let message = ClientJsonRpcMessage::notification(ClientNotification::InitializedNotification(notification));

    let encoded = encode_tx::<RoleClient>(&message).unwrap();
    assert_eq!(
        encoded.headers.get(HEADER_MCP_METHOD).map(|value| value.as_str()),
        Some("notifications/initialized")
    );
    assert!(encoded.headers.get("Authorization").is_none());

    let decoded = decode_rx::<RoleServer>(
        Direction::Request,
        Some("notifications/initialized"),
        &encoded.headers,
        &encoded.body,
    )
    .unwrap();
    let JsonRpcMessage::Notification(notification) = decoded else {
        panic!("expected notification");
    };
    let headers = notification
        .notification
        .extensions()
        .get::<McpTransportHeaders>()
        .unwrap();
    assert_eq!(headers.get(HEADER_MCP_PROTOCOL_VERSION), Some("2026-07-28"));
    assert_eq!(headers.get(HEADER_MCP_METHOD), Some("notifications/initialized"));
    assert_eq!(headers.get("Authorization"), None);
}
