use super::Bridge;
use crate::nats::{self, FlushClient, PublishClient, RequestClient, SubscribeClient, agent};
use agent_client_protocol::{Error, Result, SetSessionModeRequest, SetSessionModeResponse};
use std::time::Instant;
use tracing::{info, instrument};

#[instrument(
    name = "acp.session.set_mode",
    skip(bridge, args),
    fields(session_id = %args.session_id, mode_id = %args.mode_id)
)]
pub async fn handle<N: SubscribeClient + RequestClient + PublishClient + FlushClient>(
    bridge: &Bridge<N>,
    args: SetSessionModeRequest,
) -> Result<SetSessionModeResponse> {
    let start = Instant::now();

    info!(session_id = %args.session_id, mode_id = %args.mode_id, "Set session mode request");

    let nats = bridge.require_nats()?;
    let subject = agent::session_set_mode(&bridge.acp_prefix, &args.session_id.to_string());

    let result =
        nats::request::<N, SetSessionModeRequest, SetSessionModeResponse>(nats, &subject, &args)
            .await
            .map_err(|e| Error::new(-32603, e.to_string()));

    bridge.metrics.record_request(
        "set_session_mode",
        start.elapsed().as_secs_f64(),
        result.is_ok(),
    );

    result
}
