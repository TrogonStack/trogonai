use acp_nats::AcpPrefix;
use acp_nats::nats::AcpStream;
use tracing::info;
use trogon_nats::jetstream::JetStreamContext;

/// The ACP streams the host creates: the ones the JetStream prompt path cannot
/// run without. `Bridge::prompt` publishes each prompt into `COMMANDS` and
/// waits on a consumer of `RESPONSES`; the host's agent side consumes
/// `COMMANDS` and answers into `RESPONSES`.
///
/// The other ACP streams are deliberately left alone. `GLOBAL`, `GLOBAL_EXT`
/// and `CLIENT_OPS` capture subjects that are also used for core request/reply
/// (`initialize`, `session/new`, `session/request_permission`, `fs/*`). When a
/// stream captures a request's subject, JetStream answers that request with
/// its publish acknowledgement before the real responder can, and the
/// requester takes the acknowledgement as the reply. Creating them here would
/// break every one of those calls, so they stay for whoever owns that design.
pub const HOST_STREAMS: [AcpStream; 2] = [AcpStream::Commands, AcpStream::Responses];

#[derive(Debug, thiserror::Error)]
#[error("could not provision the {stream} stream")]
pub struct ProvisionError<Source> {
    stream: String,
    #[source]
    source: Source,
}

pub async fn provision<J: JetStreamContext>(js: &J, prefix: &AcpPrefix) -> Result<(), ProvisionError<J::Error>> {
    for stream in HOST_STREAMS {
        let config = stream.config(prefix);
        let name = config.name.clone();
        js.get_or_create_stream(config).await.map_err(|source| ProvisionError {
            stream: name.clone(),
            source,
        })?;
        info!(stream = %name, "provisioned JetStream stream");
    }
    Ok(())
}

#[cfg(test)]
mod tests;
