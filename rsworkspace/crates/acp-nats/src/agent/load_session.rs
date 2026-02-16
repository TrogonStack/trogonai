use super::Bridge;
use crate::nats::{self, agent, FlushClient, PublishClient, RequestClient, SessionReady, SubscribeClient};
use agent_client_protocol::{Error, LoadSessionRequest, LoadSessionResponse, Result};
use std::time::Instant;
use tracing::{info, instrument, warn};

#[instrument(
    name = "acp.session.load",
    skip(bridge, args),
    fields(session_id = %args.session_id)
)]
pub async fn handle<N: SubscribeClient + RequestClient + PublishClient + FlushClient>(
    bridge: &Bridge<N>,
    args: LoadSessionRequest,
) -> Result<LoadSessionResponse> {
    let start = Instant::now();

    info!(session_id = %args.session_id, "Load session request");

    let nats = bridge.require_nats()?;
    let subject = agent::session_load(&bridge.acp_prefix, &args.session_id.to_string());

    let result = nats::request::<N, LoadSessionRequest, LoadSessionResponse>(nats, &subject, &args)
        .await
        .map_err(|e| Error::new(-32603, e.to_string()));

    if result.is_ok() {
        // Publish session.ready after this handler returns.
        // The backend adds a small delay before sending notifications to ensure
        // the response has been written to stdout.
        let session_id = args.session_id.clone();
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

    bridge.metrics.record_request(
        "load_session",
        start.elapsed().as_secs_f64(),
        result.is_ok(),
    );

    result
}
