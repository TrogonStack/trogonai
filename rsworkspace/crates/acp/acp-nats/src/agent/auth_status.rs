use super::Bridge;
use super::rpc_call::jsonrpc_call;
use crate::nats::{RequestClient, global};
use agent_client_protocol::Result;
use agent_client_protocol::v1::schema::{AuthStatusRequest, AuthStatusResponse};
use tracing::info;
use trogon_std::time::GetElapsed;

pub async fn handle<N: RequestClient, C: GetElapsed, J>(
    bridge: &Bridge<N, C, J>,
    args: AuthStatusRequest,
) -> Result<AuthStatusResponse> {
    let start = bridge.clock.now();

    info!("AuthStatus request");

    let result = jsonrpc_call(
        bridge.nats(),
        &global::AuthStatusSubject::new(bridge.config.acp_prefix_ref()),
        "auth/status",
        &args,
        bridge.config.operation_timeout,
    )
    .await;

    bridge
        .metrics
        .record_request("auth_status", bridge.clock.elapsed(start).as_secs_f64(), result.is_ok());

    result
}
