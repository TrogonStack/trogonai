use super::*;

#[tokio::test]
async fn connect_subscribes_to_server_subjects() {
    let nats = trogon_nats::AdvancedMockNatsClient::new();
    let _inbound = nats.inject_messages();
    let config = Config::new(
        crate::McpPrefix::new("mcp").unwrap(),
        trogon_nats::NatsConfig {
            servers: vec!["localhost:4222".to_string()],
            auth: trogon_nats::NatsAuth::None,
        },
    );

    let _transport = connect(
        nats.clone(),
        &config,
        McpPeerId::new("filesystem").unwrap(),
        McpPeerId::new("desktop").unwrap(),
    )
    .await
    .unwrap();

    assert_eq!(nats.subscribed_to(), vec!["mcp.server.filesystem.>"]);
}
