use super::Bridge;
use crate::nats::{self, FlushClient, PublishClient, commands, parsing::SessionAgentMethod, responses};
use crate::session_id::AcpSessionId;
use crate::wire::encode_notification;
use agent_client_protocol::{CancelNotification, Error, ErrorCode, Result};
use tracing::{info, instrument, warn};
use trogon_semconv::span::ACP_SESSION_CANCEL;
use trogon_std::time::GetElapsed;

/// Handles cancel notification requests.
///
/// Validates the session ID and publishes the cancellation to the backend (fire-and-forget).
/// The backend owns session state and will respond to the in-flight prompt with `stopReason: cancelled`.
/// Publish failure is logged and recorded in metrics but does not propagate to the caller.
#[instrument(
    name = ACP_SESSION_CANCEL,
    skip(bridge, args),
    fields(session_id = %args.session_id)
)]
pub async fn handle<N: PublishClient + FlushClient, C: GetElapsed, J>(
    bridge: &Bridge<N, C, J>,
    args: CancelNotification,
) -> Result<()> {
    let start = bridge.clock.now();

    info!(session_id = %args.session_id, "Cancel notification");

    let session_id = AcpSessionId::try_from(&args.session_id).map_err(|e| {
        bridge
            .metrics
            .record_request("cancel", bridge.clock.elapsed(start).as_secs_f64(), false);
        bridge.metrics.record_error("cancel", "invalid_session_id");
        Error::new(ErrorCode::InvalidParams.into(), format!("Invalid session ID: {}", e))
    })?;

    let prefix = bridge.config.acp_prefix_ref();
    let subject = commands::CancelSubject::new(prefix, &session_id);

    let publish_result = match encode_notification(SessionAgentMethod::Cancel.wire_method(), &args) {
        Ok(encoded) => {
            nats::publish_wire(
                bridge.nats(),
                &subject,
                encoded,
                nats::PublishOptions::builder()
                    .flush_policy(nats::FlushPolicy::no_retries())
                    .build(),
            )
            .await
        }
        Err(error) => Err(trogon_nats::NatsError::Other(format!("encode cancel: {error}"))),
    };

    if let Err(error) = &publish_result {
        warn!(
            session_id = %args.session_id,
            error = %error,
            "Failed to publish cancel notification to backend"
        );
        bridge.metrics.record_error("cancel", "cancel_publish_failed");
    }

    let cancelled_subject = responses::CancelledSubject::new(prefix, &session_id);
    if let Err(e) = bridge
        .nats()
        .publish_with_headers(cancelled_subject, async_nats::HeaderMap::new(), bytes::Bytes::new())
        .await
    {
        warn!(session_id = %args.session_id, error = %e, "Failed to publish session_cancelled broadcast");
    }

    bridge.metrics.record_request(
        "cancel",
        bridge.clock.elapsed(start).as_secs_f64(),
        publish_result.is_ok(),
    );

    Ok(())
}

#[cfg(test)]
mod tests;
