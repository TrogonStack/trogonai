use crate::constants::HOST_STREAMS;
use acp_nats::AcpPrefix;
use acp_nats::jetstream::provision::{ProvisionError, provision_selected_streams};
use async_nats::jetstream::context::GetStreamError;
use trogon_nats::jetstream::{JetStreamContext, JetStreamGetStream, JetStreamGetStreamInfo};

pub async fn provision<J>(js: &J, prefix: &AcpPrefix) -> Result<(), ProvisionError<<J as JetStreamContext>::Error>>
where
    J: JetStreamContext + JetStreamGetStream<Error = GetStreamError>,
    <J as JetStreamGetStream>::Stream: JetStreamGetStreamInfo,
{
    provision_selected_streams(js, prefix, HOST_STREAMS).await
}

#[cfg(test)]
mod tests;
