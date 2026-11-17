use async_nats::jetstream::{context::GetStreamError, stream::InfoError};
use tracing::info;
use trogon_nats::jetstream::{
    JetStreamContext, JetStreamGetStream, JetStreamGetStreamInfo, ProvisionedStreamField, is_get_stream_not_found,
};

use super::streams;
use crate::acp_prefix::AcpPrefix;

#[derive(Debug, thiserror::Error)]
pub enum ProvisionError<Source> {
    #[error("could not inspect stream {stream}")]
    Inspect {
        stream: String,
        #[source]
        source: GetStreamError,
    },
    #[error("could not read configuration for stream {stream}")]
    Info {
        stream: String,
        #[source]
        source: InfoError,
    },
    #[error("stream {stream} has no exclusive subject ownership for ACP prefix {}", .prefix.as_str())]
    OwnershipMismatch { stream: String, prefix: AcpPrefix },
    #[error("stream provisioning failed for {stream}")]
    Reconcile {
        stream: String,
        #[source]
        source: Source,
    },
}

pub async fn provision_streams<J>(
    js: &J,
    prefix: &AcpPrefix,
) -> Result<(), ProvisionError<<J as JetStreamContext>::Error>>
where
    J: JetStreamContext + JetStreamGetStream<Error = GetStreamError>,
    <J as JetStreamGetStream>::Stream: JetStreamGetStreamInfo,
{
    let configs = streams::all_configs(prefix);
    let namespace = format!("{}.v1.", prefix.as_str());
    // Stream names normalize distinct prefixes, so check every owner before any write.
    for config in &configs {
        let current = match js.get_stream(&config.name).await {
            Ok(current) => current,
            Err(source) if is_get_stream_not_found(&source) => continue,
            Err(source) => {
                return Err(ProvisionError::Inspect {
                    stream: config.name.clone(),
                    source,
                });
            }
        };
        let info = current.get_info().await.map_err(|source| ProvisionError::Info {
            stream: config.name.clone(),
            source,
        })?;
        if info.config.subjects.is_empty()
            || !info
                .config
                .subjects
                .iter()
                .all(|subject| subject.starts_with(&namespace))
        {
            return Err(ProvisionError::OwnershipMismatch {
                stream: config.name.clone(),
                prefix: prefix.clone(),
            });
        }
    }
    for config in configs {
        let name = config.name.clone();
        js.create_or_reconcile_stream(config, &[ProvisionedStreamField::Subjects])
            .await
            .map_err(|source| ProvisionError::Reconcile {
                stream: name.clone(),
                source,
            })?;
        info!(stream = %name, "Provisioned JetStream stream");
    }
    Ok(())
}

#[cfg(test)]
mod tests;
