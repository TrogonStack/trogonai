use async_nats::jetstream::{context::GetStreamError, stream::InfoError};
use tracing::info;
use trogon_nats::jetstream::{
    JetStreamContext, JetStreamGetStream, JetStreamGetStreamInfo, ProvisionedStreamField, is_get_stream_not_found,
};

use crate::acp_prefix::AcpPrefix;
use crate::nats::{AcpStream, AcpSubjectNamespace};

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
    provision_selected_streams(js, prefix, AcpStream::ALL).await
}

/// Creates or reconciles only the given streams, in order. Every existing
/// stream's ownership is checked before any write, and the first failure stops.
pub async fn provision_selected_streams<J>(
    js: &J,
    prefix: &AcpPrefix,
    streams: impl IntoIterator<Item = AcpStream>,
) -> Result<(), ProvisionError<<J as JetStreamContext>::Error>>
where
    J: JetStreamContext + JetStreamGetStream<Error = GetStreamError>,
    <J as JetStreamGetStream>::Stream: JetStreamGetStreamInfo,
{
    let configs: Vec<_> = streams.into_iter().map(|stream| stream.config(prefix)).collect();
    let namespace = AcpSubjectNamespace::new(prefix);
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
        if info.config.subjects.is_empty() || !info.config.subjects.iter().all(|subject| namespace.contains(subject)) {
            return Err(ProvisionError::OwnershipMismatch {
                stream: config.name.clone(),
                prefix: prefix.clone(),
            });
        }
    }
    for config in configs {
        let name = config.name.clone();
        let owned = js
            .create_or_reconcile_stream_if_owned(config, &[ProvisionedStreamField::Subjects], namespace.as_str())
            .await
            .map_err(|source| ProvisionError::Reconcile {
                stream: name.clone(),
                source,
            })?;
        if !owned {
            return Err(ProvisionError::OwnershipMismatch {
                stream: name,
                prefix: prefix.clone(),
            });
        }
        info!(stream = %name, "Provisioned JetStream stream");
    }
    Ok(())
}

#[cfg(test)]
mod tests;
