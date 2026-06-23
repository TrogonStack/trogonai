use tracing::info;
use trogon_nats::jetstream::JetStreamContext;

use super::stream_options::StreamProvisionOptions;
use super::streams;
use crate::a2a_prefix::A2aPrefix;

#[derive(Debug, thiserror::Error)]
#[error("stream provisioning failed: {0}")]
pub struct ProvisionError(pub String);

pub async fn provision_streams<J: JetStreamContext>(js: &J, prefix: &A2aPrefix) -> Result<(), ProvisionError> {
    provision_streams_with_options(js, prefix, &StreamProvisionOptions::default()).await
}

pub async fn provision_streams_with_options<J: JetStreamContext>(
    js: &J,
    prefix: &A2aPrefix,
    options: &StreamProvisionOptions,
) -> Result<(), ProvisionError> {
    for config in streams::all_configs_with_options(prefix, options) {
        let name = config.name.clone();
        js.get_or_create_stream(config)
            .await
            .map_err(|e| ProvisionError(format!("{name}: {e}")))?;
        info!(stream = %name, "Provisioned JetStream stream");
    }
    Ok(())
}

#[cfg(test)]
mod tests;
