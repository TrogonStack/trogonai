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
mod tests;
