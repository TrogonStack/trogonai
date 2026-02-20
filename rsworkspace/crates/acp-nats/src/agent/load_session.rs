use super::Bridge;
use crate::JSONRPC_INTERNAL_ERROR;
use crate::nats::{self, FlushClient, PublishClient, RequestClient, SubscribeClient, agent};
use agent_client_protocol::{Error, LoadSessionRequest, LoadSessionResponse, Result};
use std::time::Instant;
use tracing::{info, instrument};

#[instrument(
    name = "acp.session.load",
    skip(bridge, args),
    fields(session_id = %args.session_id)
)]
pub async fn handle<N: SubscribeClient + RequestClient + PublishClient + FlushClient>(
    bridge: &Bridge<N>,
    args: LoadSessionRequest,
) -> Result<LoadSessionResponse> {
    let start = Instant::now();

    info!(session_id = %args.session_id, "Load session request");

    let nats = bridge.require_nats()?;
    let subject = agent::session_load(&bridge.acp_prefix, &args.session_id.to_string());

    let result = nats::request::<N, LoadSessionRequest, LoadSessionResponse>(nats, &subject, &args)
        .await
        .map_err(|e| Error::new(JSONRPC_INTERNAL_ERROR, e.to_string()));

    if result.is_ok() {
        bridge.spawn_session_ready(nats, &args.session_id);
    }

    bridge.metrics.record_request(
        "load_session",
        start.elapsed().as_secs_f64(),
        result.is_ok(),
    );

    result
}
