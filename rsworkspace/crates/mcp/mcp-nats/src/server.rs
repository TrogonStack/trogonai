use rmcp::service::RoleServer;
use trogon_nats::{FlushClient, PublishClient, RequestClient, SubscribeClient};

use crate::{Config, McpPeerId, NatsTransport, NatsTransportError};

pub async fn connect<N>(
    nats: N,
    config: &Config,
    server_id: McpPeerId,
    client_id: McpPeerId,
) -> Result<NatsTransport<RoleServer, N>, NatsTransportError>
where
    N: SubscribeClient + RequestClient + PublishClient + FlushClient,
    N::SubscribeError: 'static,
{
    NatsTransport::for_server(nats, config, server_id, client_id).await
}

#[cfg(test)]
mod tests;
