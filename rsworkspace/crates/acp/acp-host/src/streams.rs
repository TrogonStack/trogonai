use acp_nats::AcpPrefix;
use acp_nats::jetstream::provision::{ProvisionError, provision_selected_streams};
use acp_nats::nats::AcpStream;
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

pub async fn provision<J: JetStreamContext>(js: &J, prefix: &AcpPrefix) -> Result<(), ProvisionError<J::Error>> {
    provision_selected_streams(js, prefix, HOST_STREAMS).await
}

#[cfg(test)]
mod tests;
