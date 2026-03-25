use crate::agent::Bridge;
use crate::config::Config;
use opentelemetry::Value;
use opentelemetry::metrics::MeterProvider;
use opentelemetry_sdk::metrics::data::{AggregatedMetrics, MetricData};
use opentelemetry_sdk::metrics::{
    PeriodicReader, SdkMeterProvider, in_memory_exporter::InMemoryMetricExporter,
};
use std::time::Duration;
use trogon_nats::AdvancedMockNatsClient;

pub(crate) fn mock_bridge() -> (
    AdvancedMockNatsClient,
    Bridge<AdvancedMockNatsClient, trogon_std::time::SystemClock>,
) {
    let mock = AdvancedMockNatsClient::new();
    let bridge = Bridge::new(
        mock.clone(),
        trogon_std::time::SystemClock,
        &opentelemetry::global::meter("acp-nats-test"),
        Config::for_test("acp"),
        tokio::sync::mpsc::channel(1).0,
    );
    (mock, bridge)
}

pub(crate) fn mock_bridge_with_metrics() -> (
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
        tokio::sync::mpsc::channel(1).0,
    );
    (mock, bridge, exporter, provider)
}

pub(crate) fn set_json_response<T: serde::Serialize>(
    mock: &AdvancedMockNatsClient,
    subject: &str,
    resp: &T,
) {
    let bytes = serde_json::to_vec(resp).unwrap();
    mock.set_response(subject, bytes.into());
}

/// Returns `true` if `finished_metrics` contains an `acp.requests` counter data-point
/// with `method == method` and `success == expected_success`.
pub(crate) fn has_request_metric(
    finished_metrics: &[opentelemetry_sdk::metrics::data::ResourceMetrics],
    method: &str,
    expected_success: bool,
) -> bool {
    finished_metrics
        .iter()
        .flat_map(|rm| rm.scope_metrics())
        .flat_map(|sm| sm.metrics())
        .find(|m| m.name() == "acp.requests")
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

/// Returns `true` if `finished_metrics` contains an `acp.errors` counter data-point
/// with `operation == operation` and `reason == reason`.
pub(crate) fn has_error_metric(
    finished_metrics: &[opentelemetry_sdk::metrics::data::ResourceMetrics],
    operation: &str,
    reason: &str,
) -> bool {
    finished_metrics
        .iter()
        .flat_map(|rm| rm.scope_metrics())
        .flat_map(|sm| sm.metrics())
        .find(|m| m.name() == "acp.errors")
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

#[cfg(test)]
mod tests {
    use super::*;
    use opentelemetry::metrics::MeterProvider;

    #[test]
    fn has_request_metric_returns_false_when_metric_is_histogram() {
        let exporter = InMemoryMetricExporter::default();
        let reader = PeriodicReader::builder(exporter.clone())
            .with_interval(Duration::from_millis(100))
            .build();
        let provider = SdkMeterProvider::builder().with_reader(reader).build();
        let meter = provider.meter("test");
        let histogram = meter
            .f64_histogram("acp.requests")
            .with_description("test")
            .build();
        histogram.record(1.0, &[]);
        provider.force_flush().unwrap();
        let finished_metrics = exporter.get_finished_metrics().unwrap();
        assert!(!has_request_metric(&finished_metrics, "any_method", true));
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
            .f64_histogram("acp.errors")
            .with_description("test")
            .build();
        histogram.record(1.0, &[]);
        provider.force_flush().unwrap();
        let finished_metrics = exporter.get_finished_metrics().unwrap();
        assert!(!has_error_metric(&finished_metrics, "any_op", "any_reason"));
        provider.shutdown().unwrap();
    }
}
