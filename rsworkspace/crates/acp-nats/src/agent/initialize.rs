use super::Bridge;
use super::rpc_call::jsonrpc_call;
use crate::nats::{RequestClient, global};
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

    let subject = global::InitializeSubject::new(bridge.config.acp_prefix_ref());

    let result = jsonrpc_call(
        bridge.nats(),
        &subject,
        "initialize",
        &args,
        bridge.config.operation_timeout,
    )
    .await;

    bridge
        .metrics
        .record_request("initialize", bridge.clock.elapsed(start).as_secs_f64(), result.is_ok());

    result
}

#[cfg(test)]
mod tests;
