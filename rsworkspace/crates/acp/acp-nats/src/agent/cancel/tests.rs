use super::Bridge;
use crate::AgentHandler;
use crate::agent::test_support::{has_error_metric, has_request_metric, mock_bridge, mock_bridge_with_metrics};
use crate::config::Config;
use agent_client_protocol::ErrorCode;
use agent_client_protocol::schema::v1::CancelNotification;
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
    assert!(err.message.contains("Invalid session ID"));
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
