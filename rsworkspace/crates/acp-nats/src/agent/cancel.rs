use super::Bridge;
use crate::nats::{self, FlushClient, PublishClient, RequestClient, agent};
use agent_client_protocol::{CancelNotification, PromptResponse, Result, StopReason};
use tracing::{info, instrument, warn};
use trogon_std::time::GetElapsed;

/// Handles cancel notification requests.
///
/// Marks the session as cancelled, resolves any pending prompt waiters, and publishes
/// the cancellation to the backend. The backend publish is fire-and-forget - if it fails,
/// the error is logged and recorded in metrics, but the method still returns `Ok(())`.
/// This ensures that local state (cancelled sessions, prompt waiters) is always updated
/// even if the backend notification fails.
#[instrument(
    name = "acp.session.cancel",
    skip(bridge, args),
    fields(session_id = %args.session_id)
)]
pub async fn handle<
    N: RequestClient + PublishClient + FlushClient,
    C: GetElapsed,
>(
    bridge: &Bridge<N, C>,
    args: CancelNotification,
) -> Result<()> {
    let start = bridge.clock.now();

    info!(session_id = %args.session_id, "Cancel notification");

    bridge.validate_session(&args.session_id)?;

    bridge
        .cancelled_sessions
        .mark_cancelled(args.session_id.clone(), &bridge.clock);

    bridge
        .pending_session_prompt_responses
        .resolve_waiter(&args.session_id, Ok(PromptResponse::new(StopReason::Cancelled)));

    let subject = agent::session_cancel(&bridge.config.acp_prefix, &args.session_id.to_string());

    let publish_result = nats::publish(
        bridge.nats(),
        &subject,
        &args,
        nats::PublishOptions::builder()
            .flush_policy(nats::FlushPolicy::no_retries())
            .build(),
    )
    .await;

    let publish_ok = match publish_result {
        Ok(()) => true,
        Err(error) => {
            warn!(session_id = %args.session_id, error = %error, "Failed to publish cancel notification to backend");
            bridge.metrics.record_error("cancel", "cancel_publish_failed");
            false
        }
    };

    bridge.metrics.record_request(
        "cancel",
        bridge.clock.elapsed(start).as_secs_f64(),
        publish_ok,
    );

    Ok(())
}
