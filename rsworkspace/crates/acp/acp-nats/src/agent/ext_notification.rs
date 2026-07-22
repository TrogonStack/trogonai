use super::Bridge;
use crate::ext_method_name::ExtMethodName;
use crate::nats::{self, FlushClient, PublishClient, global};
use agent_client_protocol::schema::v1::ExtNotification;
use agent_client_protocol::{Error, ErrorCode, Result};
use tracing::{info, instrument, warn};
use trogon_semconv::span::ACP_EXT_NOTIFICATION;
use trogon_std::time::GetElapsed;

/// Handles extension notification requests (fire-and-forget).
///
/// Publishes to NATS; payload size and other validation are left to NATS.
/// Publish failure is logged and recorded as a metric but does not propagate
/// to the caller, so the client always receives `Ok(())`.
#[instrument(
    name = ACP_EXT_NOTIFICATION,
    skip(bridge, args),
    fields(method = %args.method)
)]
pub async fn handle<N: PublishClient + FlushClient, C: GetElapsed, J>(
    bridge: &Bridge<N, C, J>,
    args: ExtNotification,
) -> Result<()> {
    let start = bridge.clock.now();

    info!(method = %args.method, "Extension notification");

    let method_name = ExtMethodName::new(&args.method).map_err(|e| {
        bridge
            .metrics
            .record_request("ext_notification", bridge.clock.elapsed(start).as_secs_f64(), false);
        bridge.metrics.record_error("ext_notification", "invalid_method_name");
        Error::new(ErrorCode::InvalidParams.into(), format!("Invalid method name: {}", e))
    })?;

    let subject = global::ExtNotifySubject::new(bridge.config.acp_prefix_ref(), &method_name);

    let publish_result = nats::publish(
        bridge.nats(),
        &subject,
        &args,
        nats::PublishOptions::builder()
            .flush_policy(nats::FlushPolicy::no_retries())
            .build(),
    )
    .await;

    if let Err(error) = publish_result {
        warn!(
            method = %args.method,
            error = %error,
            "Failed to publish ext_notification to backend"
        );
        bridge
            .metrics
            .record_error("ext_notification", "ext_notification_publish_failed");
    }

    bridge
        .metrics
        .record_request("ext_notification", bridge.clock.elapsed(start).as_secs_f64(), true);

    Ok(())
}

#[cfg(test)]
mod tests;
