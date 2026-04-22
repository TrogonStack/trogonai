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

pub fn mock_bridge() -> (
    AdvancedMockNatsClient,
    MockJs,
    Bridge<AdvancedMockNatsClient, trogon_std::time::MockClock, MockJs>,
) {
    let mock = AdvancedMockNatsClient::new();
    let js = MockJs::new();
    let bridge = Bridge::new(
        mock.clone(),
        js.clone(),
        trogon_std::time::MockClock::new(),
        &opentelemetry::global::meter("acp-nats-test"),
        Config::for_test("acp"),
        tokio::sync::mpsc::channel(1).0,
    );
    (mock, js, bridge)
}

#[derive(Clone)]
pub struct MockJs {
    pub publisher: trogon_nats::jetstream::MockJetStreamPublisher,
    pub consumer_factory: trogon_nats::jetstream::MockJetStreamConsumerFactory,
}

impl MockJs {
    pub fn new() -> Self {
        Self {
            publisher: trogon_nats::jetstream::MockJetStreamPublisher::new(),
            consumer_factory: trogon_nats::jetstream::MockJetStreamConsumerFactory::new(),
        }
    }
}

impl trogon_nats::jetstream::JetStreamPublisher for MockJs {
    type PublishError = trogon_nats::mocks::MockError;
    type AckFuture =
        std::future::Ready<Result<async_nats::jetstream::publish::PublishAck, Self::PublishError>>;

    async fn publish_with_headers<S: async_nats::subject::ToSubject + Send>(
        &self,
        subject: S,
        headers: async_nats::HeaderMap,
        payload: bytes::Bytes,
    ) -> Result<Self::AckFuture, Self::PublishError> {
        self.publisher
            .publish_with_headers(subject, headers, payload)
            .await
    }
}

impl trogon_nats::jetstream::JetStreamGetStream for MockJs {
    type Error = trogon_nats::mocks::MockError;
    type Stream = trogon_nats::jetstream::MockJetStreamStream;

    async fn get_stream<T: AsRef<str> + Send>(
        &self,
        stream_name: T,
    ) -> Result<trogon_nats::jetstream::MockJetStreamStream, Self::Error> {
        self.consumer_factory.get_stream(stream_name).await
    }
}

pub fn mock_bridge_with_metrics() -> (
    AdvancedMockNatsClient,
    MockJs,
    Bridge<AdvancedMockNatsClient, trogon_std::time::MockClock, MockJs>,
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
    let js = MockJs::new();
    let bridge = Bridge::new(
        mock.clone(),
        js.clone(),
        trogon_std::time::MockClock::new(),
        &meter,
        Config::for_test("acp"),
        tokio::sync::mpsc::channel(1).0,
    );
    (mock, js, bridge, exporter, provider)
}

pub fn set_json_response<T: serde::Serialize>(
    mock: &AdvancedMockNatsClient,
    subject: &str,
    resp: &T,
) {
    let bytes = serde_json::to_vec(resp).unwrap();
    mock.set_response(subject, bytes.into());
}

pub fn set_js_raw_response(js: &MockJs, payload: &[u8]) {
    let msg = trogon_nats::jetstream::MockJsMessage::new(async_nats::Message {
        subject: "test".into(),
        reply: None,
        payload: bytes::Bytes::from(payload.to_vec()),
        headers: None,
        status: None,
        description: None,
        length: 0,
    });
    let (consumer, tx) = trogon_nats::jetstream::MockJetStreamConsumer::new();
    tx.unbounded_send(Ok(msg)).unwrap();
    js.consumer_factory.add_consumer(consumer);
}

pub fn set_js_response<T: serde::Serialize>(js: &MockJs, resp: &T) {
    let bytes = serde_json::to_vec(resp).unwrap();
    let msg = trogon_nats::jetstream::MockJsMessage::new(async_nats::Message {
        subject: "test".into(),
        reply: None,
        payload: bytes::Bytes::from(bytes),
        headers: None,
        status: None,
        description: None,
        length: 0,
    });
    let (consumer, tx) = trogon_nats::jetstream::MockJetStreamConsumer::new();
    tx.unbounded_send(Ok(msg)).unwrap();
    js.consumer_factory.add_consumer(consumer);
}

pub fn has_request_metric(
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

pub fn has_error_metric(
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

pub fn has_session_ready_error_metric(
    finished_metrics: &[opentelemetry_sdk::metrics::data::ResourceMetrics],
) -> bool {
    has_error_metric(
        finished_metrics,
        "session_ready",
        "session_ready_publish_failed",
    )
}

fn flush_metrics(
    provider: &SdkMeterProvider,
    exporter: &InMemoryMetricExporter,
) -> Vec<opentelemetry_sdk::metrics::data::ResourceMetrics> {
    provider.force_flush().unwrap();
    exporter.get_finished_metrics().unwrap()
}

fn test_provider() -> (SdkMeterProvider, InMemoryMetricExporter) {
    let exporter = InMemoryMetricExporter::default();
    let reader = PeriodicReader::builder(exporter.clone())
        .with_interval(Duration::from_millis(100))
        .build();
    let provider = SdkMeterProvider::builder().with_reader(reader).build();
    (provider, exporter)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::telemetry::metrics::Metrics;

    #[test]
    fn has_request_metric_matches_correct_method_and_success() {
        let (provider, exporter) = test_provider();
        let meter = provider.meter("test");
        let metrics = Metrics::new(&meter);
        metrics.record_request("initialize", 0.01, true);

        let finished = flush_metrics(&provider, &exporter);
        assert!(has_request_metric(&finished, "initialize", true));
        provider.shutdown().unwrap();
    }

    #[test]
    fn has_request_metric_rejects_wrong_method() {
        let (provider, exporter) = test_provider();
        let meter = provider.meter("test");
        let metrics = Metrics::new(&meter);
        metrics.record_request("initialize", 0.01, true);

        let finished = flush_metrics(&provider, &exporter);
        assert!(!has_request_metric(&finished, "authenticate", true));
        provider.shutdown().unwrap();
    }

    #[test]
    fn has_request_metric_rejects_wrong_success() {
        let (provider, exporter) = test_provider();
        let meter = provider.meter("test");
        let metrics = Metrics::new(&meter);
        metrics.record_request("initialize", 0.01, true);

        let finished = flush_metrics(&provider, &exporter);
        assert!(!has_request_metric(&finished, "initialize", false));
        provider.shutdown().unwrap();
    }

    #[test]
    fn has_request_metric_returns_false_when_empty() {
        let (provider, exporter) = test_provider();
        let finished = flush_metrics(&provider, &exporter);
        assert!(!has_request_metric(&finished, "initialize", true));
        provider.shutdown().unwrap();
    }

    #[test]
    fn has_request_metric_returns_false_for_histogram_metric() {
        let (provider, exporter) = test_provider();
        let meter = provider.meter("test");
        let histogram = meter
            .f64_histogram("acp.requests")
            .with_description("test")
            .build();
        histogram.record(1.0, &[]);

        let finished = flush_metrics(&provider, &exporter);
        assert!(!has_request_metric(&finished, "initialize", true));
        provider.shutdown().unwrap();
    }

    #[test]
    fn has_error_metric_matches_correct_operation_and_reason() {
        let (provider, exporter) = test_provider();
        let meter = provider.meter("test");
        let metrics = Metrics::new(&meter);
        metrics.record_error("session_validate", "invalid_session_id");

        let finished = flush_metrics(&provider, &exporter);
        assert!(has_error_metric(
            &finished,
            "session_validate",
            "invalid_session_id"
        ));
        provider.shutdown().unwrap();
    }

    #[test]
    fn has_error_metric_rejects_wrong_operation() {
        let (provider, exporter) = test_provider();
        let meter = provider.meter("test");
        let metrics = Metrics::new(&meter);
        metrics.record_error("session_validate", "invalid_session_id");

        let finished = flush_metrics(&provider, &exporter);
        assert!(!has_error_metric(
            &finished,
            "wrong_operation",
            "invalid_session_id"
        ));
        provider.shutdown().unwrap();
    }

    #[test]
    fn has_error_metric_rejects_wrong_reason() {
        let (provider, exporter) = test_provider();
        let meter = provider.meter("test");
        let metrics = Metrics::new(&meter);
        metrics.record_error("session_validate", "invalid_session_id");

        let finished = flush_metrics(&provider, &exporter);
        assert!(!has_error_metric(
            &finished,
            "session_validate",
            "wrong_reason"
        ));
        provider.shutdown().unwrap();
    }

    #[test]
    fn has_error_metric_returns_false_for_histogram_metric() {
        let (provider, exporter) = test_provider();
        let meter = provider.meter("test");
        let histogram = meter
            .f64_histogram("acp.errors")
            .with_description("test")
            .build();
        histogram.record(1.0, &[]);

        let finished = flush_metrics(&provider, &exporter);
        assert!(!has_error_metric(
            &finished,
            "session_validate",
            "invalid_session_id"
        ));
        provider.shutdown().unwrap();
    }

    #[test]
    fn has_session_ready_error_metric_matches() {
        let (provider, exporter) = test_provider();
        let meter = provider.meter("test");
        let metrics = Metrics::new(&meter);
        metrics.record_error("session_ready", "session_ready_publish_failed");

        let finished = flush_metrics(&provider, &exporter);
        assert!(has_session_ready_error_metric(&finished));
        provider.shutdown().unwrap();
    }

    #[test]
    fn has_session_ready_error_metric_rejects_wrong_operation() {
        let (provider, exporter) = test_provider();
        let meter = provider.meter("test");
        let metrics = Metrics::new(&meter);
        metrics.record_error("wrong_operation", "session_ready_publish_failed");

        let finished = flush_metrics(&provider, &exporter);
        assert!(!has_session_ready_error_metric(&finished));
        provider.shutdown().unwrap();
    }
}
