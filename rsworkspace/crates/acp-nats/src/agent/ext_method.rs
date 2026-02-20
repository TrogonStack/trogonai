use super::Bridge;
use crate::JSONRPC_INTERNAL_ERROR;
use crate::nats::{self, FlushClient, PublishClient, RequestClient, SubscribeClient, agent};
use agent_client_protocol::{Error, ExtRequest, ExtResponse, Result};
use std::time::Instant;
use tracing::{info, instrument};

#[instrument(
    name = "acp.ext",
    skip(bridge, args),
    fields(method = %args.method)
)]
pub async fn handle<N: SubscribeClient + RequestClient + PublishClient + FlushClient>(
    bridge: &Bridge<N>,
    args: ExtRequest,
) -> Result<ExtResponse> {
    let start = Instant::now();

    info!(method = %args.method, "Extension method request");

    let nats = bridge.require_nats()?;
    let subject = agent::ext(&bridge.acp_prefix, &args.method);

    let result = nats::request::<N, ExtRequest, ExtResponse>(nats, &subject, &args)
        .await
        .map_err(|e| Error::new(JSONRPC_INTERNAL_ERROR, e.to_string()));

    bridge
        .metrics
        .record_request("ext_method", start.elapsed().as_secs_f64(), result.is_ok());

    result
}
