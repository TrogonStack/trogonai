use super::Bridge;
use crate::error::map_nats_error;
use crate::nats::{self, RequestClient, global};
use agent_client_protocol::{InitializeRequest, InitializeResponse, Result};
use tracing::{info, instrument};
use trogon_std::time::GetElapsed;

#[instrument(
    name = "acp.initialize",
    skip(bridge, args),
    fields(protocol_version = ?args.protocol_version)
)]
pub async fn handle<N: RequestClient, C: GetElapsed, J>(
    bridge: &Bridge<N, C, J>,
    args: InitializeRequest,
) -> Result<InitializeResponse> {
    let start = bridge.clock.now();

    let client_name = args.client_info.as_ref().map(|c| c.name.as_str()).unwrap_or("unknown");

    info!(client = %client_name, "Initialize request");

    let nats = bridge.nats();
    let subject = global::InitializeSubject::new(bridge.config.acp_prefix_ref());

    let result = nats::request_with_timeout::<N, InitializeRequest, InitializeResponse>(
        nats,
        &subject,
        &args,
        bridge.config.operation_timeout,
    )
    .await
    .map_err(map_nats_error);

    bridge
        .metrics
        .record_request("initialize", bridge.clock.elapsed(start).as_secs_f64(), result.is_ok());

    result
}

#[cfg(test)]
mod tests;
