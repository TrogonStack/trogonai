use super::Bridge;
use super::error::log_and_map_nats_error;
use crate::nats::{self, FlushClient, PublishClient, RequestClient, agent};
use agent_client_protocol::{NewSessionRequest, NewSessionResponse, Result};
use tracing::{Span, info, instrument};
use trogon_std::time::GetElapsed;

#[instrument(
    name = "acp.session.new",
    skip(bridge, args),
    fields(cwd = ?args.cwd, mcp_servers = args.mcp_servers.len())
)]
pub async fn handle<
    N: RequestClient + PublishClient + FlushClient,
    C: GetElapsed,
>(
    bridge: &Bridge<N, C>,
    args: NewSessionRequest,
) -> Result<NewSessionResponse> {
    let start = bridge.clock.now();

    info!(cwd = ?args.cwd, mcp_servers = args.mcp_servers.len(), "New session request");

    let nats = bridge.nats();

    let result = nats::request_with_timeout::<N, NewSessionRequest, NewSessionResponse>(
        nats,
        &agent::session_new(&bridge.config.acp_prefix),
        &args,
        bridge.config.operation_timeout,
    )
    .await
    .map_err(log_and_map_nats_error);

    if let Ok(ref response) = result {
        Span::current().record("session_id", response.session_id.to_string().as_str());
        info!(session_id = %response.session_id, "Session created");
        bridge.metrics.record_session_created();
        bridge.spawn_session_ready(&response.session_id);
    }

    bridge.metrics.record_request(
        "new_session",
        bridge.clock.elapsed(start).as_secs_f64(),
        result.is_ok(),
    );

    result
}
