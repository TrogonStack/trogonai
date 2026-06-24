use super::Bridge;
use crate::error::map_nats_error;
use crate::nats::{self, RequestClient, global};
use agent_client_protocol::{ListSessionsRequest, ListSessionsResponse, Result};
use tracing::{info, instrument};
use trogon_std::time::GetElapsed;

#[instrument(name = "acp.session.list", skip(bridge, args))]
pub async fn handle<N: RequestClient, C: GetElapsed, J>(
    bridge: &Bridge<N, C, J>,
    args: ListSessionsRequest,
) -> Result<ListSessionsResponse> {
    let start = bridge.clock.now();

    info!("List sessions request");

    let nats = bridge.nats();
    let subject = global::SessionListSubject::new(bridge.config.acp_prefix_ref());

    let result = nats::request_with_timeout::<N, ListSessionsRequest, ListSessionsResponse>(
        nats,
        &subject,
        &args,
        bridge.config.operation_timeout,
    )
    .await
    .map_err(map_nats_error);

    bridge.metrics.record_request(
        "list_sessions",
        bridge.clock.elapsed(start).as_secs_f64(),
        result.is_ok(),
    );

    result
}

#[cfg(test)]
mod tests;
