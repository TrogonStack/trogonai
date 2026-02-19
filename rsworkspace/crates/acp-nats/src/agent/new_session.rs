use super::Bridge;
use crate::nats::{
    self, FlushClient, PublishClient, RequestClient, SessionReady, SubscribeClient, agent,
};
use agent_client_protocol::{Error, NewSessionRequest, NewSessionResponse, Result};
use std::time::Instant;
use tracing::{Span, info, instrument, warn};

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

        // Publish session.ready after this handler returns.
        // The backend adds a small delay before sending notifications to ensure
        // the response has been written to stdout.
        let session_id = response.session_id.clone();
        let nats_clone = nats.clone();
        let prefix = bridge.acp_prefix.clone();
        tokio::task::spawn_local(async move {
            let ready_subject = agent::ext_session_ready(&prefix, &session_id.to_string());
            info!(session_id = %session_id, subject = %ready_subject, "Publishing session.ready");

            let ready_message = SessionReady::new(session_id.to_string());

            let options = nats::PublishOptions::builder()
                .publish_retry_policy(nats::RetryPolicy::standard())
                .flush_policy(nats::FlushPolicy::standard())
                .build();

            if let Err(e) =
                nats::publish(&nats_clone, &ready_subject, &ready_message, options).await
            {
                warn!(
                    error = %e,
                    session_id = %session_id,
                    "Failed to publish session.ready"
                );
            } else {
                info!(session_id = %session_id, "Published session.ready");
            }
        });
    }

    bridge
        .metrics
        .record_request("new_session", start.elapsed().as_secs_f64(), result.is_ok());

    result
}
