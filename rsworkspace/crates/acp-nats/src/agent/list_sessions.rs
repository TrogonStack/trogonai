use super::Bridge;
use super::rpc_call::jsonrpc_call;
use crate::nats::{RequestClient, global};
use agent_client_protocol::{ListSessionsRequest, ListSessionsResponse, Result};
use tracing::{info, instrument};
use trogon_semconv::span::ACP_SESSION_LIST;
use trogon_std::time::GetElapsed;

#[instrument(name = ACP_SESSION_LIST, skip(bridge, args))]
pub async fn handle<N: RequestClient, C: GetElapsed, J>(
    bridge: &Bridge<N, C, J>,
    args: ListSessionsRequest,
) -> Result<ListSessionsResponse> {
    let start = bridge.clock.now();

    info!("List sessions request");

    let subject = global::SessionListSubject::new(bridge.config.acp_prefix_ref());

    let result = jsonrpc_call(
        bridge.nats(),
        &subject,
        "session.list",
        &args,
        bridge.config.operation_timeout,
    )
    .await;

    bridge.metrics.record_request(
        "list_sessions",
        bridge.clock.elapsed(start).as_secs_f64(),
        result.is_ok(),
    );

    result
}

#[cfg(test)]
mod tests;
