use super::Bridge;
use crate::nats::{self, FlushClient, PublishClient, RequestClient, SubscribeClient, agent};
use agent_client_protocol::{CancelNotification, PromptResponse, Result, StopReason};
use std::time::Instant;
use tracing::{info, instrument, warn};

#[instrument(
    name = "acp.session.cancel",
    skip(bridge, args),
    fields(session_id = %args.session_id)
)]
pub async fn handle<N: SubscribeClient + RequestClient + PublishClient + FlushClient>(
    bridge: &Bridge<N>,
    args: CancelNotification,
) -> Result<()> {
    let start = Instant::now();

    info!(session_id = %args.session_id, "Cancel notification");

    bridge
        .cancelled_sessions
        .mark_cancelled(args.session_id.clone());

    bridge
        .pending_session_prompt_responses
        .resolve_waiter(
            &args.session_id,
            PromptResponse::new(StopReason::Cancelled),
        );

    let mut success = true;

    if let Some(nats) = &bridge.nats {
        let subject = agent::session_cancel(&bridge.acp_prefix, &args.session_id.to_string());

        if let Err(e) = nats::publish(nats, &subject, &args, nats::PublishOptions::simple()).await {
            warn!(error = %e, session_id = %args.session_id, "Failed to publish cancel notification");
            success = false;
        }
    }

    bridge
        .metrics
        .record_request("cancel", start.elapsed().as_secs_f64(), success);

    Ok(())
}
