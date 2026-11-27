use super::Bridge;
use super::rpc_call::jsonrpc_call;
use crate::nats::{RequestClient, global};
use agent_client_protocol::Result;
use agent_client_protocol::v1::schema::{SessionOptionsRequest, SessionOptionsResponse};
use tracing::info;
use trogon_std::time::GetElapsed;

pub async fn handle<N: RequestClient, C: GetElapsed, J>(
    bridge: &Bridge<N, C, J>,
    args: SessionOptionsRequest,
) -> Result<SessionOptionsResponse> {
    let start = bridge.clock.now();

    info!(session_id = ?args.session_id, "SessionOptions request");

    let result = jsonrpc_call(
        bridge.nats(),
        &global::SessionOptionsSubject::new(bridge.config.acp_prefix_ref()),
        "session/options",
        &args,
        bridge.config.operation_timeout,
    )
    .await;

    bridge.metrics.record_request(
        "session_options",
        bridge.clock.elapsed(start).as_secs_f64(),
        result.is_ok(),
    );

    result
}
