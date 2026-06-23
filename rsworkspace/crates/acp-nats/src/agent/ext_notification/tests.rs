use crate::agent::test_support::{has_error_metric, has_request_metric, mock_bridge, mock_bridge_with_metrics};
use agent_client_protocol::{Agent, ErrorCode, ExtNotification};
use serde_json::value::RawValue;

#[tokio::test]
async fn ext_notification_publishes_to_nats() {
    let (mock, _js, bridge) = mock_bridge();

    let params = RawValue::from_string(r#"{"event":"ping"}"#.to_string()).unwrap();
    let notification = ExtNotification::new("my_notify", params.into());
    let result = bridge.ext_notification(notification).await;

    assert!(result.is_ok());

    let published = mock.published_messages();
    assert!(
        published.iter().any(|s| s.contains("agent.ext.my_notify")),
        "Expected ext notification publish, got: {:?}",
        published
    );
}

#[tokio::test]
async fn ext_notification_validates_method_name() {
    let (_mock, _js, bridge) = mock_bridge();
    let params = RawValue::from_string("{}".to_string()).unwrap();
    let notification = ExtNotification::new("method.*", params.into());
    let err = bridge.ext_notification(notification).await.unwrap_err();

    assert!(err.message.contains("Invalid method name"));
    assert_eq!(err.code, ErrorCode::InvalidParams);
}

#[tokio::test]
async fn ext_notification_records_request_metric_on_invalid_method_name() {
    let (_mock, _js, bridge, exporter, provider) = mock_bridge_with_metrics();

    let _ = bridge
        .ext_notification(ExtNotification::new(
            "method.*",
            RawValue::from_string("{}".to_string()).unwrap().into(),
        ))
        .await;

    provider.force_flush().unwrap();
    let finished_metrics = exporter.get_finished_metrics().unwrap();
    assert!(
        has_request_metric(&finished_metrics, "ext_notification", false),
        "expected acp.requests with method=ext_notification, success=false on validation failure"
    );
    assert!(
        has_error_metric(&finished_metrics, "ext_notification", "invalid_method_name"),
        "expected acp.errors with operation=ext_notification, reason=invalid_method_name"
    );
    provider.shutdown().unwrap();
}

#[tokio::test]
async fn ext_notification_records_metrics_on_success() {
    let (_mock, _js, bridge, exporter, provider) = mock_bridge_with_metrics();

    let _ = bridge
        .ext_notification(ExtNotification::new(
            "my_notify",
            RawValue::from_string("{}".to_string()).unwrap().into(),
        ))
        .await;

    provider.force_flush().unwrap();
    let finished_metrics = exporter.get_finished_metrics().unwrap();
    assert!(
        has_request_metric(&finished_metrics, "ext_notification", true),
        "expected acp.requests with method=ext_notification, success=true"
    );
    provider.shutdown().unwrap();
}

#[tokio::test]
async fn ext_notification_returns_ok_when_publish_fails() {
    let (mock, _js, bridge) = mock_bridge();
    mock.fail_publish_count(1);

    let params = RawValue::from_string("{}".to_string()).unwrap();
    let notification = ExtNotification::new("my_notify", params.into());
    let result = bridge.ext_notification(notification).await;

    assert!(result.is_ok(), "fire-and-forget: caller always gets Ok(())");
}

#[tokio::test]
async fn ext_notification_records_error_metric_on_publish_failure() {
    let (mock, _js, bridge, exporter, provider) = mock_bridge_with_metrics();
    mock.fail_publish_count(1);

    let _ = bridge
        .ext_notification(ExtNotification::new(
            "my_notify",
            RawValue::from_string("{}".to_string()).unwrap().into(),
        ))
        .await;

    provider.force_flush().unwrap();
    let finished_metrics = exporter.get_finished_metrics().unwrap();
    assert!(
        has_error_metric(&finished_metrics, "ext_notification", "ext_notification_publish_failed"),
        "expected acp.errors with operation=ext_notification, reason=ext_notification_publish_failed"
    );
    assert!(
        has_request_metric(&finished_metrics, "ext_notification", true),
        "publish failure is fire-and-forget; caller still gets Ok, so success=true"
    );
    provider.shutdown().unwrap();
}
