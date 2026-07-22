use tracing::info;
use trogon_nats::jetstream::{JetStreamContext, JetStreamStreamUpdater};

use super::streams;

#[derive(Debug, thiserror::Error)]
#[error("stream provisioning failed for {stream}")]
pub struct ProvisionError {
    stream: String,
    #[source]
    source: Box<dyn std::error::Error + Send + Sync>,
}

pub async fn provision_streams<J>(js: &J, prefix: &crate::acp_prefix::AcpPrefix) -> Result<(), ProvisionError>
where
    J: JetStreamContext + JetStreamStreamUpdater,
    <J as JetStreamContext>::Error: 'static,
    <J as JetStreamStreamUpdater>::UpdateError: 'static,
{
    for config in streams::all_configs(prefix) {
        let name = config.name.clone();
        js.get_or_create_stream(config.clone())
            .await
            .map_err(|source| ProvisionError {
                stream: name.clone(),
                source: Box::new(source),
            })?;
        js.update_stream(config).await.map_err(|source| ProvisionError {
            stream: name.clone(),
            source: Box::new(source),
        })?;
        info!(stream = %name, "Provisioned JetStream stream");
    }
    Ok(())
}

#[cfg(test)]
mod tests;
