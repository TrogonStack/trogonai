use super::Bridge;
use crate::JSONRPC_INTERNAL_ERROR;
use crate::nats::{self, FlushClient, PublishClient, RequestClient, SubscribeClient, agent};
use agent_client_protocol::{Error, InitializeRequest, InitializeResponse, Result};
use std::time::Instant;
use tracing::{info, instrument};

#[instrument(
    name = "acp.initialize",
    skip(bridge, args),
    fields(protocol_version = ?args.protocol_version)
)]
pub async fn handle<N: SubscribeClient + RequestClient + PublishClient + FlushClient>(
    bridge: &Bridge<N>,
    args: InitializeRequest,
) -> Result<InitializeResponse> {
    let start = Instant::now();

    let client_name = args
        .client_info
        .as_ref()
        .map(|c| c.name.as_str())
        .unwrap_or("unknown");

    info!(client = %client_name, "Initialize request");

    let nats = bridge.require_nats()?;

    let result = nats::request::<N, InitializeRequest, InitializeResponse>(
        nats,
        &agent::initialize(&bridge.acp_prefix),
        &args,
    )
    .await
    .map_err(|e| Error::new(JSONRPC_INTERNAL_ERROR, e.to_string()));

    bridge
        .metrics
        .record_request("initialize", start.elapsed().as_secs_f64(), result.is_ok());

    result
}
