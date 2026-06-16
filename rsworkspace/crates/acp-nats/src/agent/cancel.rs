use super::Bridge;
use crate::nats::{self, FlushClient, PublishClient, session};
use crate::session_id::AcpSessionId;
use agent_client_protocol::{CancelNotification, Error, ErrorCode, Result};
use tracing::{info, instrument, warn};
use trogon_std::time::GetElapsed;

/// Handles cancel notification requests.
///
/// Validates the session ID and publishes the cancellation to the backend (fire-and-forget).
/// The backend owns session state and will respond to the in-flight prompt with `stopReason: cancelled`.
/// Publish failure is logged and recorded in metrics but does not propagate to the caller.
#[instrument(
    name = "acp.session.cancel",
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
    let subject = session::agent::CancelSubject::new(prefix, &session_id);

    let publish_result = nats::publish(
        bridge.nats(),
        &subject,
        &args,
        nats::PublishOptions::builder()
            .flush_policy(nats::FlushPolicy::no_retries())
            .build(),
    )
    .await;

    if let Err(error) = &publish_result {
        warn!(
            session_id = %args.session_id,
            error = %error,
            "Failed to publish cancel notification to backend"
        );
        bridge.metrics.record_error("cancel", "cancel_publish_failed");
    }

    let cancelled_subject = session::agent::CancelledSubject::new(prefix, &session_id);
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
mod tests {
    use super::Bridge;
    use crate::agent::test_support::{has_error_metric, has_request_metric, mock_bridge, mock_bridge_with_metrics};
    use crate::config::Config;
    use agent_client_protocol::{Agent, CancelNotification, ErrorCode};
    use trogon_nats::AdvancedMockNatsClient;
    use trogon_std::time::MockClock;

    fn mock_bridge_with_clock() -> (
        AdvancedMockNatsClient,
        MockClock,
        Bridge<AdvancedMockNatsClient, MockClock, crate::agent::test_support::MockJs>,
    ) {
        let mock = AdvancedMockNatsClient::new();
        let clock = MockClock::new();
        let bridge = Bridge::new(
            mock.clone(),
            crate::agent::test_support::MockJs::new(),
            clock.clone(),
            &opentelemetry::global::meter("acp-nats-test"),
            Config::for_test("acp"),
            tokio::sync::mpsc::channel(1).0,
        );
        (mock, clock, bridge)
    }

    #[tokio::test]
    async fn cancel_publishes_to_correct_subject() {
        let (mock, _js, bridge) = mock_bridge();

        let _ = bridge.cancel(CancelNotification::new("s1")).await;

        let published = mock.published_messages();
        assert!(
            published.contains(&"acp.session.s1.agent.cancel".to_string()),
            "expected publish to acp.session.s1.agent.cancel, got: {:?}",
            published
        );
    }

    #[tokio::test]
    async fn cancel_also_publishes_session_cancelled_broadcast() {
        let (mock, _js, bridge) = mock_bridge();

        let _ = bridge.cancel(CancelNotification::new("s1")).await;

        let published = mock.published_messages();
        assert!(
            published.contains(&"acp.session.s1.agent.cancelled".to_string()),
            "expected publish to acp.session.s1.agent.cancelled (prompt broadcast), got: {:?}",
            published
        );
    }

    #[tokio::test]
    async fn cancel_validates_session_id() {
        let (_mock, _js, bridge) = mock_bridge();
        let err = bridge
            .cancel(CancelNotification::new("invalid.session.id"))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("Invalid session ID"));
        assert_eq!(err.code, ErrorCode::InvalidParams);
    }

    #[tokio::test]
    async fn cancel_records_request_metric_on_invalid_session_id() {
        let (_mock, _js, bridge, exporter, provider) = mock_bridge_with_metrics();

        let _ = bridge.cancel(CancelNotification::new("invalid.session.id")).await;

        provider.force_flush().unwrap();
        let finished_metrics = exporter.get_finished_metrics().unwrap();
        assert!(
            has_request_metric(&finished_metrics, "cancel", false),
            "expected acp.requests with method=cancel, success=false on validation failure"
        );
        assert!(
            has_error_metric(&finished_metrics, "cancel", "invalid_session_id"),
            "expected acp.errors with operation=cancel, reason=invalid_session_id"
        );
        provider.shutdown().unwrap();
    }

    #[tokio::test]
    async fn cancel_records_metrics_on_success() {
        let (_mock, _js, bridge, exporter, provider) = mock_bridge_with_metrics();

        let _ = bridge.cancel(CancelNotification::new("s1")).await;

        provider.force_flush().unwrap();
        let finished_metrics = exporter.get_finished_metrics().unwrap();
        assert!(
            has_request_metric(&finished_metrics, "cancel", true),
            "expected acp.requests with method=cancel, success=true"
        );
        provider.shutdown().unwrap();
    }

    #[tokio::test]
    async fn cancel_records_error_metric_on_publish_failure() {
        let (mock, _js, bridge, exporter, provider) = mock_bridge_with_metrics();
        mock.fail_publish_count(1);

        let _ = bridge.cancel(CancelNotification::new("s1")).await;

        provider.force_flush().unwrap();
        let finished_metrics = exporter.get_finished_metrics().unwrap();
        assert!(
            has_error_metric(&finished_metrics, "cancel", "cancel_publish_failed"),
            "expected acp.errors with operation=cancel, reason=cancel_publish_failed"
        );
        assert!(
            has_request_metric(&finished_metrics, "cancel", false),
            "request metric records publish outcome; success=false when publish fails"
        );
        provider.shutdown().unwrap();
    }

    #[tokio::test]
    async fn cancel_publishes_to_nats() {
        let (mock, _clock, bridge) = mock_bridge_with_clock();
        let session_id = "cancel-session-003";

        let notification = CancelNotification::new(session_id);
        bridge.cancel(notification).await.unwrap();

        let published = mock.published_messages();
        assert!(
            published.iter().any(|s| s.contains("session.cancel")),
            "Expected cancel publish, got: {:?}",
            published
        );
    }
}
