use super::Bridge;
use crate::error::map_nats_error;
use crate::nats::{self, RequestClient, global};
use agent_client_protocol::{LogoutRequest, LogoutResponse, Result};
use tracing::{info, instrument};
use trogon_std::time::GetElapsed;

#[instrument(name = "acp.logout", skip(bridge, args))]
pub async fn handle<N: RequestClient, C: GetElapsed, J>(
    bridge: &Bridge<N, C, J>,
    args: LogoutRequest,
) -> Result<LogoutResponse> {
    let start = bridge.clock.now();

    info!("Logout request");
    let nats = bridge.nats();

    let result = nats::request_with_timeout::<N, LogoutRequest, LogoutResponse>(
        nats,
        &global::LogoutSubject::new(bridge.config.acp_prefix_ref()),
        &args,
        bridge.config.operation_timeout,
    )
    .await
    .map_err(map_nats_error);

    bridge
        .metrics
        .record_request("logout", bridge.clock.elapsed(start).as_secs_f64(), result.is_ok());

    result
}

#[cfg(test)]
mod tests;
