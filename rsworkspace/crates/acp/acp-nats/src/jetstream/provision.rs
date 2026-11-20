use tracing::info;
use trogon_nats::jetstream::JetStreamContext;

use crate::nats::AcpStream;

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
    provision_selected_streams(js, prefix, AcpStream::ALL).await
}

/// Creates only the given streams, in order, stopping at the first failure.
pub async fn provision_selected_streams<J: JetStreamContext>(
    js: &J,
    prefix: &crate::acp_prefix::AcpPrefix,
    streams: impl IntoIterator<Item = AcpStream>,
) -> Result<(), ProvisionError<J::Error>> {
    for config in streams.into_iter().map(|stream| stream.config(prefix)) {
        let name = config.name.clone();
        js.get_or_create_stream(config).await.map_err(|source| ProvisionError {
            stream: name.clone(),
            source,
        })?;
        info!(stream = %name, "Provisioned JetStream stream");
    }
    Ok(())
}

#[cfg(test)]
mod tests;
