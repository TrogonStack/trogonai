use super::Bridge;
use crate::ext_method_name::ExtMethodName;
use crate::nats::{self, FlushClient, PublishClient, RequestClient, agent};
use agent_client_protocol::{Error, ErrorCode, ExtNotification, Result};
use tracing::{info, instrument, warn};
use trogon_std::time::GetElapsed;

/// Handles extension notification requests (fire-and-forget).
///
/// Publishes to NATS; payload size and other validation are left to NATS.
/// Publish failure is logged and recorded as a metric but does not propagate
/// to the caller, so the client always receives `Ok(())`.
#[instrument(
    name = "acp.ext.notification",
    skip(bridge, args),
    fields(method = %args.method)
)]
pub async fn handle<N: RequestClient + PublishClient + FlushClient, C: GetElapsed>(
    bridge: &Bridge<N, C>,
    args: ExtNotification,
) -> Result<()> {
    let start = bridge.clock.now();

    info!(method = %args.method, "Extension notification");

    let method_name = ExtMethodName::new(&args.method).map_err(|e| {
        bridge.metrics.record_request(
            "ext_notification",
            bridge.clock.elapsed(start).as_secs_f64(),
            false,
        );
        bridge
            .metrics
            .record_error("ext_notification", "invalid_method_name");
        Error::new(
            ErrorCode::InvalidParams.into(),
            format!("Invalid method name: {}", e),
        )
    })?;

    let subject = agent::ext(bridge.config.acp_prefix(), method_name.as_str());

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

    bridge.metrics.record_request(
        "ext_notification",
        bridge.clock.elapsed(start).as_secs_f64(),
        true,
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use agent_client_protocol::{Agent, ErrorCode, ExtNotification};
    use opentelemetry::Value;
    use opentelemetry::metrics::MeterProvider;
    use opentelemetry_sdk::metrics::data::{AggregatedMetrics, MetricData};
    use opentelemetry_sdk::metrics::{
        PeriodicReader, SdkMeterProvider, in_memory_exporter::InMemoryMetricExporter,
    };
    use serde_json::value::RawValue;
    use std::time::Duration;
    use trogon_nats::AdvancedMockNatsClient;

    fn mock_bridge() -> (
        AdvancedMockNatsClient,
        Bridge<AdvancedMockNatsClient, trogon_std::time::SystemClock>,
    ) {
        let mock = AdvancedMockNatsClient::new();
        let bridge = Bridge::new(
            mock.clone(),
            trogon_std::time::SystemClock,
            &opentelemetry::global::meter("acp-nats-test"),
            Config::for_test("acp"),
        );
        (mock, bridge)
    }

    fn mock_bridge_with_metrics() -> (
        AdvancedMockNatsClient,
        Bridge<AdvancedMockNatsClient, trogon_std::time::SystemClock>,
        InMemoryMetricExporter,
        SdkMeterProvider,
    ) {
        let exporter = InMemoryMetricExporter::default();
        let reader = PeriodicReader::builder(exporter.clone())
            .with_interval(Duration::from_millis(100))
            .build();
        let provider = SdkMeterProvider::builder().with_reader(reader).build();
        let meter = provider.meter("acp-nats-test");

        let mock = AdvancedMockNatsClient::new();
        let bridge = Bridge::new(
            mock.clone(),
            trogon_std::time::SystemClock,
            &meter,
            Config::for_test("acp"),
        );
        (mock, bridge, exporter, provider)
    }

    fn has_request_metric(
        finished_metrics: &[opentelemetry_sdk::metrics::data::ResourceMetrics],
        method: &str,
        expected_success: bool,
    ) -> bool {
        finished_metrics
            .iter()
            .flat_map(|rm| rm.scope_metrics())
            .flat_map(|sm| sm.metrics())
            .find(|m| m.name() == "acp.request.count")
            .and_then(|metric| {
                let data = metric.data();
                if let AggregatedMetrics::U64(MetricData::Sum(s)) = data {
                    s.data_points()
                        .find(|dp| {
                            let mut method_ok = false;
                            let mut success_ok = false;
                            for attr in dp.attributes() {
                                if attr.key.as_str() == "method" {
                                    method_ok = attr.value.as_str() == method;
                                } else if attr.key.as_str() == "success" {
                                    success_ok = attr.value == Value::from(expected_success);
                                }
                            }
                            method_ok && success_ok
                        })
                        .map(|_| ())
                } else {
                    None
                }
            })
            .is_some()
    }

    fn has_error_metric(
        finished_metrics: &[opentelemetry_sdk::metrics::data::ResourceMetrics],
        operation: &str,
        reason: &str,
    ) -> bool {
        finished_metrics
            .iter()
            .flat_map(|rm| rm.scope_metrics())
            .flat_map(|sm| sm.metrics())
            .find(|m| m.name() == "acp.errors.total")
            .and_then(|metric| {
                let data = metric.data();
                if let AggregatedMetrics::U64(MetricData::Sum(s)) = data {
                    s.data_points()
                        .find(|dp| {
                            let mut operation_ok = false;
                            let mut reason_ok = false;
                            for attr in dp.attributes() {
                                if attr.key.as_str() == "operation" {
                                    operation_ok = attr.value.as_str() == operation;
                                } else if attr.key.as_str() == "reason" {
                                    reason_ok = attr.value.as_str() == reason;
                                }
                            }
                            operation_ok && reason_ok
                        })
                        .map(|_| ())
                } else {
                    None
                }
            })
            .is_some()
    }

    #[tokio::test]
    async fn ext_notification_publishes_to_nats() {
        let (mock, bridge) = mock_bridge();

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
        let (_mock, bridge) = mock_bridge();
        let params = RawValue::from_string("{}".to_string()).unwrap();
        let notification = ExtNotification::new("method.*", params.into());
        let err = bridge.ext_notification(notification).await.unwrap_err();

        assert!(err.message.contains("Invalid method name"));
        assert_eq!(err.code, ErrorCode::InvalidParams);
    }

    #[tokio::test]
    async fn ext_notification_records_request_metric_on_invalid_method_name() {
        let (_mock, bridge, exporter, provider) = mock_bridge_with_metrics();

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
            "expected acp.request.count with method=ext_notification, success=false on validation failure"
        );
        assert!(
            has_error_metric(&finished_metrics, "ext_notification", "invalid_method_name"),
            "expected acp.errors.total with operation=ext_notification, reason=invalid_method_name"
        );
        provider.shutdown().unwrap();
    }

    #[tokio::test]
    async fn ext_notification_records_metrics_on_success() {
        let (_mock, bridge, exporter, provider) = mock_bridge_with_metrics();

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
            "expected acp.request.count with method=ext_notification, success=true"
        );
        provider.shutdown().unwrap();
    }

    #[tokio::test]
    async fn ext_notification_returns_ok_when_publish_fails() {
        let (mock, bridge) = mock_bridge();
        mock.fail_publish_count(1);

        let params = RawValue::from_string("{}".to_string()).unwrap();
        let notification = ExtNotification::new("my_notify", params.into());
        let result = bridge.ext_notification(notification).await;

        assert!(result.is_ok(), "fire-and-forget: caller always gets Ok(())");
    }

    #[tokio::test]
    async fn ext_notification_records_error_metric_on_publish_failure() {
        let (mock, bridge, exporter, provider) = mock_bridge_with_metrics();
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
            has_error_metric(
                &finished_metrics,
                "ext_notification",
                "ext_notification_publish_failed"
            ),
            "expected acp.errors.total with operation=ext_notification, reason=ext_notification_publish_failed"
        );
        assert!(
            has_request_metric(&finished_metrics, "ext_notification", true),
            "publish failure is fire-and-forget; caller still gets Ok, so success=true"
        );
        provider.shutdown().unwrap();
    }

    #[test]
    fn has_request_metric_returns_false_when_metric_is_histogram() {
        let exporter = InMemoryMetricExporter::default();
        let reader = PeriodicReader::builder(exporter.clone())
            .with_interval(Duration::from_millis(100))
            .build();
        let provider = SdkMeterProvider::builder().with_reader(reader).build();
        let meter = provider.meter("test");
        let histogram = meter
            .f64_histogram("acp.request.count")
            .with_description("test")
            .build();
        histogram.record(1.0, &[]);
        provider.force_flush().unwrap();
        let finished_metrics = exporter.get_finished_metrics().unwrap();
        assert!(!has_request_metric(
            &finished_metrics,
            "ext_notification",
            true
        ));
        provider.shutdown().unwrap();
    }

    #[test]
    fn has_error_metric_returns_false_when_metric_is_histogram() {
        let exporter = InMemoryMetricExporter::default();
        let reader = PeriodicReader::builder(exporter.clone())
            .with_interval(Duration::from_millis(100))
            .build();
        let provider = SdkMeterProvider::builder().with_reader(reader).build();
        let meter = provider.meter("test");
        let histogram = meter
            .f64_histogram("acp.errors.total")
            .with_description("test")
            .build();
        histogram.record(1.0, &[]);
        provider.force_flush().unwrap();
        let finished_metrics = exporter.get_finished_metrics().unwrap();
        assert!(!has_error_metric(
            &finished_metrics,
            "ext_notification",
            "ext_notification_publish_failed"
        ));
        provider.shutdown().unwrap();
    }
}
