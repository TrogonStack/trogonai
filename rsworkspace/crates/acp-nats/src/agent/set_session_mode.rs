use super::Bridge;
use super::error::log_and_map_nats_error;
use crate::nats::{self, FlushClient, PublishClient, RequestClient, agent};
use agent_client_protocol::{Result, SetSessionModeRequest, SetSessionModeResponse};
use tracing::{info, instrument};
use trogon_std::time::GetElapsed;

#[instrument(
    name = "acp.session.set_mode",
    skip(bridge, args),
    fields(session_id = %args.session_id, mode_id = %args.mode_id)
)]
pub async fn handle<
    N: RequestClient + PublishClient + FlushClient,
    C: GetElapsed,
>(
    bridge: &Bridge<N, C>,
    args: SetSessionModeRequest,
) -> Result<SetSessionModeResponse> {
    let start = bridge.clock.now();

    info!(session_id = %args.session_id, mode_id = %args.mode_id, "Set session mode request");

    let session_id = bridge.validate_session(&args.session_id)?;
    let nats = bridge.nats();
    let subject = agent::session_set_mode(&bridge.config.acp_prefix, session_id.as_str());

    let result = nats::request_with_timeout::<N, SetSessionModeRequest, SetSessionModeResponse>(
        nats,
        &subject,
        &args,
        bridge.config.operation_timeout,
    )
    .await
    .map_err(log_and_map_nats_error);

    bridge.metrics.record_request(
        "set_session_mode",
        bridge.clock.elapsed(start).as_secs_f64(),
        result.is_ok(),
    );

    result
}
