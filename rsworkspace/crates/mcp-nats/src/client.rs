use rmcp::service::RoleClient;
use trogon_nats::{FlushClient, PublishClient, RequestClient, SubscribeClient};

use crate::{Config, McpPeerId, NatsTransport, NatsTransportError};

pub async fn connect<N>(
    nats: N,
    config: &Config,
    client_id: McpPeerId,
    server_id: McpPeerId,
) -> Result<NatsTransport<RoleClient, N>, NatsTransportError>
where
    N: SubscribeClient + RequestClient + PublishClient + FlushClient,
    N::SubscribeError: 'static,
{
    NatsTransport::for_client(nats, config, client_id, server_id).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn connect_subscribes_to_client_subjects() {
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
            McpPeerId::new("desktop").unwrap(),
            McpPeerId::new("filesystem").unwrap(),
        )
        .await
        .unwrap();

        assert_eq!(nats.subscribed_to(), vec!["mcp.client.desktop.>"]);
    }
}
