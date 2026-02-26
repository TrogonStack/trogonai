use super::Bridge;
use crate::nats::{self, FlushClient, PublishClient, RequestClient, agent};
use crate::session_id::AcpSessionId;
use agent_client_protocol::{CancelNotification, Error, ErrorCode, Result};
use tracing::{info, instrument, warn};
use trogon_std::time::GetElapsed;

/// Publishes the cancel notification to the backend via NATS (fire-and-forget).
/// The publish failure is logged and recorded as a metric but does not propagate
/// to the caller, so the client always receives `Ok(())`.
#[instrument(
    name = "acp.session.cancel",
    skip(bridge, args),
    fields(session_id = %args.session_id)
)]
pub async fn handle<N: RequestClient + PublishClient + FlushClient, C: GetElapsed>(
    bridge: &Bridge<N, C>,
    args: CancelNotification,
) -> Result<()> {
    let start = bridge.clock.now();

    info!(session_id = %args.session_id, "Cancel notification");

    AcpSessionId::try_from(&args.session_id).map_err(|e| {
        bridge
            .metrics
            .record_request("cancel", bridge.clock.elapsed(start).as_secs_f64(), false);
        bridge
            .metrics
            .record_error("session_validate", "invalid_session_id");
        Error::new(
            ErrorCode::InvalidParams.into(),
            format!("Invalid session ID: {}", e),
        )
    })?;

    let subject = agent::session_cancel(bridge.config.acp_prefix(), &args.session_id.to_string());

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
            session_id = %args.session_id,
            error = %error,
            "Failed to publish cancel notification to backend"
        );
        bridge
            .metrics
            .record_error("cancel", "cancel_publish_failed");
    }

    bridge
        .metrics
        .record_request("cancel", bridge.clock.elapsed(start).as_secs_f64(), true);

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::Bridge;
    use crate::config::Config;
    use agent_client_protocol::{Agent, CancelNotification, ErrorCode};
    use opentelemetry::Value;
    use opentelemetry::metrics::MeterProvider;
    use opentelemetry_sdk::metrics::data::{AggregatedMetrics, MetricData};
    use opentelemetry_sdk::metrics::{
        PeriodicReader, SdkMeterProvider, in_memory_exporter::InMemoryMetricExporter,
    };
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
    async fn cancel_publishes_to_correct_subject() {
        let (mock, bridge) = mock_bridge();

        let _ = bridge.cancel(CancelNotification::new("s1")).await;

        let published = mock.published_messages();
        assert!(
            published.contains(&"acp.s1.agent.session.cancel".to_string()),
            "expected publish to acp.s1.agent.session.cancel, got: {:?}",
            published
        );
    }

    #[tokio::test]
    async fn cancel_validates_session_id() {
        let (_mock, bridge) = mock_bridge();
        let err = bridge
            .cancel(CancelNotification::new("invalid.session.id"))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("Invalid session ID"));
        assert_eq!(err.code, ErrorCode::InvalidParams);
    }

    #[tokio::test]
    async fn cancel_records_request_metric_on_invalid_session_id() {
        let (_mock, bridge, exporter, provider) = mock_bridge_with_metrics();

        let _ = bridge
            .cancel(CancelNotification::new("invalid.session.id"))
            .await;

        provider.force_flush().unwrap();
        let finished_metrics = exporter.get_finished_metrics().unwrap();
        assert!(
            has_request_metric(&finished_metrics, "cancel", false),
            "expected acp.request.count with method=cancel, success=false on validation failure"
        );
        assert!(
            has_error_metric(&finished_metrics, "session_validate", "invalid_session_id"),
            "expected acp.errors.total with operation=session_validate, reason=invalid_session_id"
        );
        provider.shutdown().unwrap();
    }

    #[tokio::test]
    async fn cancel_records_metrics_on_success() {
        let (_mock, bridge, exporter, provider) = mock_bridge_with_metrics();

        let _ = bridge.cancel(CancelNotification::new("s1")).await;

        provider.force_flush().unwrap();
        let finished_metrics = exporter.get_finished_metrics().unwrap();
        assert!(
            has_request_metric(&finished_metrics, "cancel", true),
            "expected acp.request.count with method=cancel, success=true"
        );
        provider.shutdown().unwrap();
    }

    #[tokio::test]
    async fn cancel_records_error_metric_on_publish_failure() {
        let (mock, bridge, exporter, provider) = mock_bridge_with_metrics();
        mock.fail_publish_count(1);

        let _ = bridge.cancel(CancelNotification::new("s1")).await;

        provider.force_flush().unwrap();
        let finished_metrics = exporter.get_finished_metrics().unwrap();
        assert!(
            has_error_metric(&finished_metrics, "cancel", "cancel_publish_failed"),
            "expected acp.errors.total with operation=cancel, reason=cancel_publish_failed"
        );
        assert!(
            has_request_metric(&finished_metrics, "cancel", true),
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
        assert!(!has_request_metric(&finished_metrics, "cancel", true));
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
            "cancel",
            "cancel_publish_failed"
        ));
        provider.shutdown().unwrap();
    }
}
