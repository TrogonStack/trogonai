use mcp_nats::{Config, McpPeerId, McpPrefix, client};
use rmcp::model::{
    ClientJsonRpcMessage, ClientRequest, ListToolsRequest, PaginatedRequestParams, RequestId, ServerJsonRpcMessage,
    ServerResult,
};
use rmcp::transport::Transport;
use trogon_nats::AdvancedMockNatsClient;

fn config() -> Config {
    Config::new(
        McpPrefix::new("mcp").unwrap(),
        trogon_nats::NatsConfig {
            servers: vec!["localhost:4222".to_string()],
            auth: trogon_nats::NatsAuth::None,
        },
    )
}

#[tokio::test]
async fn public_client_transport_routes_rmcp_request_over_nats() {
    let nats = AdvancedMockNatsClient::new();
    let _inbound = nats.inject_messages();
    nats.set_response(
        "mcp.server.filesystem.tools.list",
        serde_json::to_vec(&ServerJsonRpcMessage::response(
            ServerResult::empty(()),
            RequestId::Number(1),
        ))
        .unwrap()
        .into(),
    );

    let mut transport = client::connect(
        nats,
        &config(),
        McpPeerId::new("desktop").unwrap(),
        McpPeerId::new("filesystem").unwrap(),
    )
    .await
    .unwrap();

    let request = ClientRequest::ListToolsRequest(ListToolsRequest {
        method: Default::default(),
        params: Some(PaginatedRequestParams::default()),
        extensions: Default::default(),
    });
    transport
        .send(ClientJsonRpcMessage::request(request, RequestId::Number(1)))
        .await
        .unwrap();

    assert!(matches!(
        transport.receive().await.unwrap(),
        ServerJsonRpcMessage::Response(_)
    ));
}
