use super::Bridge;
use crate::nats::{self, FlushClient, PublishClient, RequestClient, SubscribeClient, agent};
use agent_client_protocol::{Error, NewSessionRequest, NewSessionResponse, Result};
use std::time::Instant;
use tracing::{Span, info, instrument};

#[instrument(
    name = "acp.session.new",
    skip(bridge, args),
    fields(cwd = ?args.cwd, mcp_servers = args.mcp_servers.len())
)]
pub async fn handle<N: SubscribeClient + RequestClient + PublishClient + FlushClient>(
    bridge: &Bridge<N>,
    args: NewSessionRequest,
) -> Result<NewSessionResponse> {
    let start = Instant::now();

    info!(cwd = ?args.cwd, mcp_servers = args.mcp_servers.len(), "New session request");

    let nats = bridge.require_nats()?;

    let result = nats::request::<N, NewSessionRequest, NewSessionResponse>(
        nats,
        &agent::session_new(&bridge.acp_prefix),
        &args,
    )
    .await
    .map_err(|e| Error::new(-32603, e.to_string()));

    if let Ok(ref response) = result {
        Span::current().record("session_id", response.session_id.to_string().as_str());
        info!(session_id = %response.session_id, "Session created");
        bridge.metrics.record_session_created();
        bridge.spawn_session_ready(nats, &response.session_id);
    }

    bridge
        .metrics
        .record_request("new_session", start.elapsed().as_secs_f64(), result.is_ok());

    result
}
