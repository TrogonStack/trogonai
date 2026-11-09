use tracing::info;
use trogon_nats::jetstream::JetStreamContext;

use crate::config::ResolvedConfig;
use crate::source_plugin;

pub(crate) async fn provision<C: JetStreamContext>(client: &C, config: &ResolvedConfig) -> Result<(), C::Error> {
    crate::credential::stream::provision(client).await?;
    info!(
        stream = crate::credential::constants::CREDENTIAL_STREAM,
        "credential stream provisioned"
    );

    // Discord is gateway-WebSocket, not a webhook source; it doesn't fit `SourcePlugin`.
    if let Some(ref cfg) = config.discord {
        crate::source::discord::provision(client, cfg).await?;
        info!(source = "discord", "stream provisioned");
    }
    // Google Pub/Sub is pulled over gRPC, not a webhook source; it doesn't fit `SourcePlugin`.
    for integration in &config.google_pubsub {
        crate::source::google_pubsub::provision(client, &integration.config).await?;
        info!(source = "google_pubsub", integration = %integration.id, "stream provisioned");
    }
    source_plugin::provision_webhook_sources(client, config).await
}

#[cfg(test)]
mod tests;
