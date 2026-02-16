use opentelemetry::KeyValue;
use opentelemetry::metrics::{Counter, Histogram, Meter};
use std::sync::OnceLock;

static METER: OnceLock<Meter> = OnceLock::new();

fn get_meter() -> &'static Meter {
    METER.get_or_init(|| opentelemetry::global::meter("acp-io-bridge-nats"))
}

pub struct Metrics {
    requests_total: Counter<u64>,
    request_duration: Histogram<f64>,
    sessions_created: Counter<u64>,
    errors_total: Counter<u64>,
}

impl Metrics {
    pub fn new() -> Self {
        let meter = get_meter();

        Self {
            requests_total: meter
                .u64_counter("acp.requests.total")
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
            errors_total: meter
                .u64_counter("acp.errors.total")
                .with_description("Total number of errors")
                .build(),
        }
    }

    pub fn record_request(&self, method: &str, duration: f64, success: bool) {
        let attrs = &[
            KeyValue::new("method", method.to_string()),
            KeyValue::new("success", success.to_string()),
        ];
        self.requests_total.add(1, attrs);
        self.request_duration.record(duration, attrs);
        if !success {
            self.errors_total
                .add(1, &[KeyValue::new("method", method.to_string())]);
        }
    }

    pub fn record_session_created(&self) {
        self.sessions_created.add(1, &[]);
    }

    pub fn record_error(&self, reason: &str) {
        self.errors_total
            .add(1, &[KeyValue::new("reason", reason.to_string())]);
    }
}

impl Default for Metrics {
    fn default() -> Self {
        Self::new()
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

    // Helper function to create test metrics with a local meter provider
    fn create_test_metrics() -> (Metrics, InMemoryMetricExporter, SdkMeterProvider) {
        let exporter = InMemoryMetricExporter::default();
        let reader = PeriodicReader::builder(exporter.clone())
            .with_interval(Duration::from_millis(100))
            .build();

        let provider = SdkMeterProvider::builder().with_reader(reader).build();

        // Create a local meter instead of using global
        let meter = provider.meter("acp-io-bridge-nats-test");

        let metrics = Metrics {
            requests_total: meter
                .u64_counter("acp.requests.total")
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
            errors_total: meter
                .u64_counter("acp.errors.total")
                .with_description("Total number of errors")
                .build(),
        };

        (metrics, exporter, provider)
    }

    #[tokio::test]
    async fn test_metrics_record_request_success() {
        let (metrics, exporter, provider) = create_test_metrics();

        // Record a successful request
        metrics.record_request("test_method", 1.5, true);

        // Force flush to ensure metrics are exported
        provider.force_flush().unwrap();

        // Retrieve finished metrics
        let finished_metrics = exporter.get_finished_metrics().unwrap();

        // Verify metrics were recorded
        assert!(!finished_metrics.is_empty(), "Should have recorded metrics");

        // Cleanup
        provider.shutdown().unwrap();
    }

    #[tokio::test]
    async fn test_metrics_record_request_failure() {
        let (metrics, exporter, provider) = create_test_metrics();

        // Record failed request (success=false should increment error counter)
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

        metrics.record_error("connection_timeout");

        provider.force_flush().unwrap();

        let finished_metrics = exporter.get_finished_metrics().unwrap();
        assert!(!finished_metrics.is_empty());

        provider.shutdown().unwrap();
    }

    #[tokio::test]
    async fn test_metrics_multiple_recordings() {
        let (metrics, exporter, provider) = create_test_metrics();

        // Record multiple operations
        metrics.record_request("operation1", 1.0, true);
        metrics.record_request("operation2", 2.0, false);
        metrics.record_session_created();
        metrics.record_error("test_error");

        provider.force_flush().unwrap();

        let finished_metrics = exporter.get_finished_metrics().unwrap();
        assert!(
            !finished_metrics.is_empty(),
            "Should have recorded multiple metrics"
        );

        provider.shutdown().unwrap();
    }
}
