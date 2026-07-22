use super::Bridge;
use super::rpc_call::jsonrpc_call;
use crate::nats::{RequestClient, global};
use agent_client_protocol::Result;
use agent_client_protocol::schema::v1::{LogoutRequest, LogoutResponse};
use tracing::{info, instrument};
use trogon_semconv::span::ACP_LOGOUT;
use trogon_std::time::GetElapsed;

#[instrument(name = ACP_LOGOUT, skip(bridge, args))]
pub async fn handle<N: RequestClient, C: GetElapsed, J>(
    bridge: &Bridge<N, C, J>,
    args: LogoutRequest,
) -> Result<LogoutResponse> {
    let start = bridge.clock.now();

    info!("Logout request");

    let result = jsonrpc_call(
        bridge.nats(),
        &global::LogoutSubject::new(bridge.config.acp_prefix_ref()),
        "logout",
        &args,
        bridge.config.operation_timeout,
    )
    .await;

    bridge
        .metrics
        .record_request("logout", bridge.clock.elapsed(start).as_secs_f64(), result.is_ok());

    result
}

#[cfg(test)]
mod tests;
