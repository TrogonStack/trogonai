use opentelemetry::KeyValue;
use opentelemetry::metrics::{Counter, Histogram, Meter};

#[derive(Clone)]
pub struct Metrics {
    requests_total: Counter<u64>,
    request_duration: Histogram<f64>,
    errors_total: Counter<u64>,
}

impl Metrics {
    pub fn new(meter: &Meter) -> Self {
        Self {
            requests_total: meter
                .u64_counter("acp.request.count")
                .with_description("Total number of ACP requests")
                .build(),
            request_duration: meter
                .f64_histogram("acp.request.duration")
                .with_description("Duration of ACP requests in seconds")
                .with_unit("s")
                .build(),
            errors_total: meter
                .u64_counter("acp.errors.total")
                .with_description("Total number of errors by operation and reason")
                .build(),
        }
    }

    pub fn record_request(&self, method: &'static str, duration: f64, success: bool) {
        let attrs = &[
            KeyValue::new("method", method),
            KeyValue::new("success", success),
        ];
        self.requests_total.add(1, attrs);
        self.request_duration.record(duration, attrs);
    }

    pub fn record_error(&self, operation: &'static str, reason: &'static str) {
        self.errors_total.add(
            1,
            &[
                KeyValue::new("operation", operation),
                KeyValue::new("reason", reason),
            ],
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opentelemetry::Value;
    use opentelemetry::metrics::MeterProvider;
    use opentelemetry_sdk::metrics::data::{AggregatedMetrics, MetricData};
    use opentelemetry_sdk::metrics::{
        PeriodicReader, SdkMeterProvider, in_memory_exporter::InMemoryMetricExporter,
    };
    use std::time::Duration;

    fn make_metrics() -> (Metrics, InMemoryMetricExporter, SdkMeterProvider) {
        let exporter = InMemoryMetricExporter::default();
        let reader = PeriodicReader::builder(exporter.clone())
            .with_interval(Duration::from_millis(100))
            .build();
        let provider = SdkMeterProvider::builder().with_reader(reader).build();
        let meter = provider.meter("acp-nats-metrics-test");
        (Metrics::new(&meter), exporter, provider)
    }

    /// `record_request` must increment `acp.request.count` with the correct
    /// `method` and `success` attributes.
    #[test]
    fn record_request_increments_counter_with_method_and_success() {
        let (metrics, exporter, provider) = make_metrics();

        metrics.record_request("initialize", 0.5, true);

        provider.force_flush().unwrap();
        let finished = exporter.get_finished_metrics().unwrap();

        let found = finished
            .iter()
            .flat_map(|rm| rm.scope_metrics())
            .flat_map(|sm| sm.metrics())
            .any(|m| {
                if m.name() != "acp.request.count" {
                    return false;
                }
                let AggregatedMetrics::U64(MetricData::Sum(s)) = m.data() else {
                    return false;
                };
                s.data_points().any(|dp| {
                    let mut method_ok = false;
                    let mut success_ok = false;
                    for attr in dp.attributes() {
                        if attr.key.as_str() == "method" {
                            method_ok = attr.value.as_str() == "initialize";
                        } else if attr.key.as_str() == "success" {
                            success_ok = attr.value == Value::Bool(true);
                        }
                    }
                    method_ok && success_ok && dp.value() == 1
                })
            });

        assert!(found, "acp.request.count must be recorded with method=initialize, success=true");
        provider.shutdown().unwrap();
    }

    /// `record_request` must record a data point in the `acp.request.duration`
    /// histogram with the correct `method` and `success` attributes.
    /// This is the histogram that the existing `initialize.rs` tests never verify.
    #[test]
    fn record_request_records_duration_histogram() {
        let (metrics, exporter, provider) = make_metrics();

        metrics.record_request("initialize", 1.23, false);

        provider.force_flush().unwrap();
        let finished = exporter.get_finished_metrics().unwrap();

        let found = finished
            .iter()
            .flat_map(|rm| rm.scope_metrics())
            .flat_map(|sm| sm.metrics())
            .any(|m| {
                if m.name() != "acp.request.duration" {
                    return false;
                }
                let AggregatedMetrics::F64(MetricData::Histogram(h)) = m.data() else {
                    return false;
                };
                h.data_points().any(|dp| {
                    let mut method_ok = false;
                    let mut success_ok = false;
                    for attr in dp.attributes() {
                        if attr.key.as_str() == "method" {
                            method_ok = attr.value.as_str() == "initialize";
                        } else if attr.key.as_str() == "success" {
                            success_ok = attr.value == Value::Bool(false);
                        }
                    }
                    // The recorded value 1.23 must fall within the histogram's
                    // tracked range (sum == 1.23 is the most direct check).
                    method_ok && success_ok && (dp.sum() - 1.23_f64).abs() < 1e-9
                })
            });

        assert!(
            found,
            "acp.request.duration must be recorded with method=initialize, success=false, sum≈1.23"
        );
        provider.shutdown().unwrap();
    }

    /// Calling `record_request` multiple times accumulates count in the counter.
    #[test]
    fn record_request_accumulates_multiple_calls() {
        let (metrics, exporter, provider) = make_metrics();

        metrics.record_request("initialize", 0.1, true);
        metrics.record_request("initialize", 0.2, true);
        metrics.record_request("initialize", 0.3, true);

        provider.force_flush().unwrap();
        let finished = exporter.get_finished_metrics().unwrap();

        let total: u64 = finished
            .iter()
            .flat_map(|rm| rm.scope_metrics())
            .flat_map(|sm| sm.metrics())
            .filter(|m| m.name() == "acp.request.count")
            .flat_map(|m| {
                if let AggregatedMetrics::U64(MetricData::Sum(s)) = m.data() {
                    s.data_points()
                        .filter(|dp| {
                            dp.attributes().any(|a| {
                                a.key.as_str() == "success" && a.value == Value::Bool(true)
                            })
                        })
                        .map(|dp| dp.value())
                        .collect::<Vec<_>>()
                } else {
                    vec![]
                }
            })
            .sum();

        assert_eq!(total, 3, "three record_request calls must sum to count=3");
        provider.shutdown().unwrap();
    }
}
