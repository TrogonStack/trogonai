use opentelemetry::KeyValue;
use opentelemetry::metrics::{Counter, Histogram, Meter};

#[derive(Clone)]
pub struct Metrics {
    requests: Counter<u64>,
    request_duration: Histogram<f64>,
    sessions_created: Counter<u64>,
    errors: Counter<u64>,
}

impl Metrics {
    pub fn new(meter: &Meter) -> Self {
        Self {
            requests: meter
                .u64_counter("acp.requests")
                .with_description("Total number of ACP requests")
                .build(),
            request_duration: meter
                .f64_histogram("acp.request.duration")
                .with_description("Duration of ACP requests in seconds")
                .with_unit("s")
                .build(),
            sessions_created: meter
                .u64_counter("acp.sessions.created")
                .with_description("Total number of sessions created")
                .build(),
            errors: meter
                .u64_counter("acp.errors")
                .with_description("Total number of errors by operation and reason")
                .build(),
        }
    }

    pub fn record_request(&self, method: &'static str, duration: f64, success: bool) {
        let attrs = &[
            KeyValue::new("method", method),
            KeyValue::new("success", success),
        ];
        self.requests.add(1, attrs);
        self.request_duration.record(duration, attrs);
    }

    pub fn record_session_created(&self) {
        self.sessions_created.add(1, &[]);
    }

    pub fn record_error(&self, operation: &'static str, reason: &'static str) {
        self.errors.add(
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
    use opentelemetry::metrics::MeterProvider;
    use opentelemetry_sdk::metrics::{
        PeriodicReader, SdkMeterProvider, in_memory_exporter::InMemoryMetricExporter,
    };
    use std::time::Duration;

    fn create_test_metrics() -> (Metrics, InMemoryMetricExporter, SdkMeterProvider) {
        let exporter = InMemoryMetricExporter::default();
        let reader = PeriodicReader::builder(exporter.clone())
            .with_interval(Duration::from_millis(100))
            .build();

        let provider = SdkMeterProvider::builder().with_reader(reader).build();

        let meter = provider.meter("acp-io-bridge-nats-test");

        let metrics = Metrics::new(&meter);

        (metrics, exporter, provider)
    }

    #[tokio::test]
    async fn test_metrics_record_request_success() {
        let (metrics, exporter, provider) = create_test_metrics();

        metrics.record_request("test_method", 1.5, true);

        provider.force_flush().unwrap();

        let finished_metrics = exporter.get_finished_metrics().unwrap();

        assert!(!finished_metrics.is_empty(), "Should have recorded metrics");

        provider.shutdown().unwrap();
    }

    #[tokio::test]
    async fn test_metrics_record_request_failure() {
        let (metrics, exporter, provider) = create_test_metrics();

        metrics.record_request("failing_method", 0.5, false);

        provider.force_flush().unwrap();

        let finished_metrics = exporter.get_finished_metrics().unwrap();
        assert!(!finished_metrics.is_empty());

        provider.shutdown().unwrap();
    }

    #[tokio::test]
    async fn test_metrics_record_session_created() {
        let (metrics, exporter, provider) = create_test_metrics();

        metrics.record_session_created();

        provider.force_flush().unwrap();

        let finished_metrics = exporter.get_finished_metrics().unwrap();
        assert!(!finished_metrics.is_empty());

        provider.shutdown().unwrap();
    }

    #[tokio::test]
    async fn test_metrics_record_error() {
        let (metrics, exporter, provider) = create_test_metrics();

        metrics.record_error("prompt", "connection_timeout");

        provider.force_flush().unwrap();

        let finished_metrics = exporter.get_finished_metrics().unwrap();
        assert!(!finished_metrics.is_empty());

        provider.shutdown().unwrap();
    }

    #[tokio::test]
    async fn test_metrics_multiple_recordings() {
        let (metrics, exporter, provider) = create_test_metrics();

        metrics.record_request("operation1", 1.0, true);
        metrics.record_request("operation2", 2.0, false);
        metrics.record_session_created();
        metrics.record_error("ext", "test_error");

        provider.force_flush().unwrap();

        let finished_metrics = exporter.get_finished_metrics().unwrap();
        assert!(
            !finished_metrics.is_empty(),
            "Should have recorded multiple metrics"
        );

        provider.shutdown().unwrap();
    }
}
