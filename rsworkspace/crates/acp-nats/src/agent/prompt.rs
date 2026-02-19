use super::Bridge;
use crate::nats::{self, FlushClient, PublishClient, RequestClient, SubscribeClient, agent};
use agent_client_protocol::{Error, PromptRequest, PromptResponse, Result, StopReason};
use std::time::Duration;
use std::time::Instant;
use tracing::{info, instrument, warn};

// TODO: This is a temporary fix to ensure the backend always responds.
//  There is a Draft spec around `session/resume` but I am not sure how the whole thing works.
// Related to https://github.com/orgs/agentclientprotocol/discussions/396
const PROMPT_TIMEOUT: Duration = Duration::from_secs(7200); // 2 hours

#[instrument(
    name = "acp.session.prompt",
    skip(bridge, args),
    fields(session_id = %args.session_id)
)]
pub async fn handle<N: SubscribeClient + RequestClient + PublishClient + FlushClient>(
    bridge: &Bridge<N>,
    args: PromptRequest,
) -> Result<PromptResponse> {
    let start = Instant::now();

    info!(session_id = %args.session_id, "Prompt request");

    if bridge.cancelled_sessions.is_cancelled(&args.session_id) {
        bridge
            .cancelled_sessions
            .clear_cancellation(&args.session_id);
        info!(session_id = %args.session_id, "Prompt cancelled before start");
        bridge
            .metrics
            .record_request("prompt", start.elapsed().as_secs_f64(), true);
        return Ok(PromptResponse::new(StopReason::Cancelled));
    }

    let nats = bridge.require_nats()?;
    let subject = agent::session_prompt(&bridge.acp_prefix, &args.session_id.to_string());

    let rx = bridge
        .pending_session_prompt_responses
        .register_waiter(args.session_id.clone());

    // Re-check after registration to close the race window where a cancel
    // arrives between the initial check and waiter registration.
    if bridge.cancelled_sessions.is_cancelled(&args.session_id) {
        bridge
            .pending_session_prompt_responses
            .remove_waiter(&args.session_id);
        bridge
            .cancelled_sessions
            .clear_cancellation(&args.session_id);
        info!(session_id = %args.session_id, "Prompt cancelled before publish");
        bridge
            .metrics
            .record_request("prompt", start.elapsed().as_secs_f64(), true);
        return Ok(PromptResponse::new(StopReason::Cancelled));
    }

    if let Err(e) = nats::publish(nats, &subject, &args, nats::PublishOptions::simple()).await {
        bridge
            .pending_session_prompt_responses
            .remove_waiter(&args.session_id);
        warn!(session_id = %args.session_id, error = %e, "Failed to publish prompt request");
        return Err(Error::new(
            -32603,
            format!("Failed to publish prompt request: {}", e),
        ));
    }

    let result = match tokio::time::timeout(PROMPT_TIMEOUT, rx).await {
        Ok(Ok(response)) => Ok(response),
        Ok(Err(_)) => {
            warn!(session_id = %args.session_id, "Prompt response channel closed unexpectedly");
            bridge.metrics.record_error("prompt_channel_closed");
            Ok(PromptResponse::new(StopReason::EndTurn))
        }
        Err(_) => {
            warn!(session_id = %args.session_id, "Prompt request timed out");
            Ok(PromptResponse::new(StopReason::EndTurn))
        }
    };

    bridge
        .pending_session_prompt_responses
        .remove_waiter(&args.session_id);

    if let Ok(ref response) = result {
        info!(session_id = %args.session_id, stop_reason = ?response.stop_reason, "Prompt completed");
    }

    bridge
        .metrics
        .record_request("prompt", start.elapsed().as_secs_f64(), result.is_ok());

    result
}
