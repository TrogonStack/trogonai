use super::Bridge;
use crate::error::map_nats_error;
use crate::nats::{self, RequestClient, global};
use agent_client_protocol::{AuthenticateRequest, AuthenticateResponse, Result};
use tracing::{info, instrument};
use trogon_std::time::GetElapsed;

#[instrument(
    name = "acp.authenticate",
    skip(bridge, args),
    fields(method_id = %args.method_id)
)]
pub async fn handle<N: RequestClient, C: GetElapsed, J>(
    bridge: &Bridge<N, C, J>,
    args: AuthenticateRequest,
) -> Result<AuthenticateResponse> {
    let start = bridge.clock.now();

    info!(method_id = %args.method_id, "Authenticate request");
    let nats = bridge.nats();

    let result = nats::request_with_timeout::<N, AuthenticateRequest, AuthenticateResponse>(
        nats,
        &global::AuthenticateSubject::new(bridge.config.acp_prefix_ref()),
        &args,
        bridge.config.operation_timeout,
    )
    .await
    .map_err(map_nats_error);

    bridge.metrics.record_request(
        "authenticate",
        bridge.clock.elapsed(start).as_secs_f64(),
        result.is_ok(),
    );

    result
}

#[cfg(test)]
mod tests;
