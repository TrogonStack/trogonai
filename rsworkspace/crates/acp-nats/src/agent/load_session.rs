use super::Bridge;
use super::error::log_and_map_nats_error;
use crate::nats::{self, FlushClient, PublishClient, RequestClient, agent};
use agent_client_protocol::{LoadSessionRequest, LoadSessionResponse, Result};
use tracing::{info, instrument};
use trogon_std::time::GetElapsed;

#[instrument(
    name = "acp.session.load",
    skip(bridge, args),
    fields(session_id = %args.session_id)
)]
pub async fn handle<
    N: RequestClient + PublishClient + FlushClient,
    C: GetElapsed,
>(
    bridge: &Bridge<N, C>,
    args: LoadSessionRequest,
) -> Result<LoadSessionResponse> {
    let start = bridge.clock.now();

    info!(session_id = %args.session_id, "Load session request");

    let session_id = bridge.validate_session(&args.session_id)?;
    let nats = bridge.nats();
    let subject = agent::session_load(&bridge.config.acp_prefix, session_id.as_str());

    let result = nats::request_with_timeout::<N, LoadSessionRequest, LoadSessionResponse>(
        nats,
        &subject,
        &args,
        bridge.config.operation_timeout,
    )
    .await
    .map_err(log_and_map_nats_error);

    if result.is_ok() {
        bridge.spawn_session_ready(&args.session_id);
    }

    bridge.metrics.record_request(
        "load_session",
        bridge.clock.elapsed(start).as_secs_f64(),
        result.is_ok(),
    );

    result
}
