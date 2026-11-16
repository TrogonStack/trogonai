use tracing::info;
use trogon_nats::jetstream::{JetStreamContext, ProvisionedStreamField};

use super::streams;

#[derive(Debug, thiserror::Error)]
#[error("stream provisioning failed for {stream}")]
pub struct ProvisionError<Source> {
    stream: String,
    #[source]
    source: Source,
}

pub async fn provision_streams<J: JetStreamContext>(
    js: &J,
    prefix: &crate::acp_prefix::AcpPrefix,
) -> Result<(), ProvisionError<J::Error>> {
    for config in streams::all_configs(prefix) {
        let name = config.name.clone();
        js.create_or_reconcile_stream(config, &[ProvisionedStreamField::Subjects])
            .await
            .map_err(|source| ProvisionError {
                stream: name.clone(),
                source,
            })?;
        info!(stream = %name, "Provisioned JetStream stream");
    }
    Ok(())
}

#[cfg(test)]
mod tests;
