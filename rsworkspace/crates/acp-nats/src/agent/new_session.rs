use super::Bridge;
use crate::error::map_nats_error;
use crate::nats::{self, FlushClient, PublishClient, RequestClient, global};
use agent_client_protocol::{NewSessionRequest, NewSessionResponse, Result};
use tracing::{Span, info, instrument};
use trogon_std::time::GetElapsed;

#[instrument(
    name = "acp.session.new",
    skip(bridge, args),
    fields(cwd = ?args.cwd, mcp_servers = args.mcp_servers.len(), session_id = tracing::field::Empty)
)]
pub async fn handle<N: RequestClient + PublishClient + FlushClient, C: GetElapsed, J>(
    bridge: &Bridge<N, C, J>,
    args: NewSessionRequest,
) -> Result<NewSessionResponse> {
    let start = bridge.clock.now();

    info!(cwd = ?args.cwd, mcp_servers = args.mcp_servers.len(), "New session request");

    let nats = bridge.nats();
    let subject = global::SessionNewSubject::new(bridge.config.acp_prefix_ref());

    let result = nats::request_with_timeout::<N, NewSessionRequest, NewSessionResponse>(
        nats,
        &subject,
        &args,
        bridge.config.operation_timeout,
    )
    .await
    .map_err(map_nats_error);

    if let Ok(ref response) = result {
        Span::current().record("session_id", response.session_id.to_string().as_str());
        info!(session_id = %response.session_id, "Session created");

        bridge.schedule_session_ready(response.session_id.clone());
    }

    bridge
        .metrics
        .record_request("new_session", bridge.clock.elapsed(start).as_secs_f64(), result.is_ok());

    result
}

#[cfg(test)]
mod tests;
