use crate::agent::Bridge;
use crate::config::Config;
use jsonrpc_nats::{Message, ResponseId};
use opentelemetry::Value;
use opentelemetry::metrics::MeterProvider;
use opentelemetry_sdk::metrics::data::{AggregatedMetrics, MetricData};
use opentelemetry_sdk::metrics::{PeriodicReader, SdkMeterProvider, in_memory_exporter::InMemoryMetricExporter};
use std::time::Duration;
use trogon_nats::AdvancedMockNatsClient;

pub fn mock_bridge() -> (
    AdvancedMockNatsClient,
    MockJs,
    Bridge<AdvancedMockNatsClient, trogon_std::time::SystemClock, MockJs>,
) {
    let mock = AdvancedMockNatsClient::new();
    let js = MockJs::new();
    let bridge = Bridge::new(
        mock.clone(),
        js.clone(),
        trogon_std::time::SystemClock,
        &opentelemetry::global::meter("acp-nats-test"),
        Config::for_test("acp"),
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
    type AckFuture = std::future::Ready<Result<async_nats::jetstream::publish::PublishAck, Self::PublishError>>;

    async fn publish_with_headers<S: async_nats::subject::ToSubject + Send>(
        &self,
        subject: S,
        headers: async_nats::HeaderMap,
        payload: bytes::Bytes,
    ) -> Result<Self::AckFuture, Self::PublishError> {
        self.publisher.publish_with_headers(subject, headers, payload).await
    }
}

impl trogon_nats::jetstream::JetStreamGetStream for MockJs {
    type Error = async_nats::jetstream::context::GetStreamError;
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
    Bridge<AdvancedMockNatsClient, trogon_std::time::SystemClock, MockJs>,
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
        trogon_std::time::SystemClock,
        &meter,
        Config::for_test("acp"),
    );
    (mock, js, bridge, exporter, provider)
}

pub fn set_json_response<T: serde::Serialize>(mock: &AdvancedMockNatsClient, subject: &str, resp: &T) {
    set_wire_json_response(mock, subject, resp);
}

pub fn set_wire_json_response<T: serde::Serialize>(mock: &AdvancedMockNatsClient, subject: &str, resp: &T) {
    let result = serde_json::to_value(resp).unwrap();
    let encoded = jsonrpc_nats::encode(&Message::Success {
        id: ResponseId::Number(1),
        result,
    })
    .unwrap();
    mock.set_response_wire(subject, encoded.headers, encoded.body);
}

pub fn set_wire_agent_error(mock: &AdvancedMockNatsClient, subject: &str, code: i32, message: &str) {
    let encoded = jsonrpc_nats::encode(&Message::Error {
        id: ResponseId::Number(1),
        code,
        message: message.to_string(),
        data: None,
    })
    .unwrap();
    mock.set_response_wire(subject, encoded.headers, encoded.body);
}

/// Queues a reply addressed to the request but carrying an undecodable body.
pub fn set_js_raw_response(js: &MockJs, payload: &[u8]) {
    let payload = bytes::Bytes::from(payload.to_vec());
    reply_to_published_request(js, move |request_headers| {
        let mut headers = async_nats::HeaderMap::new();
        if let Some(literal) = request_headers.get(jsonrpc_nats::HEADER_ID) {
            headers.insert(jsonrpc_nats::HEADER_ID, literal.as_str());
        }
        (headers, payload)
    });
}

/// Queues a success response that echoes the request's `Jsonrpc-Id`, the way a
/// runner does.
///
/// The bridge mints the id internally, so the reply cannot be built until the
/// request has been published: response subjects are session-scoped and the
/// bridge demuxes on the header, so a reply carrying a different id is skipped.
pub fn set_js_response<T: serde::Serialize>(js: &MockJs, resp: &T) {
    let result = serde_json::to_value(resp).unwrap();
    reply_to_published_request(js, move |request_headers| {
        let encoded = jsonrpc_nats::encode(&Message::Success {
            id: crate::wire::response_id_from_request_headers(&request_headers),
            result,
        })
        .unwrap();
        (encoded.headers, encoded.body)
    });
}

fn reply_to_published_request<F>(js: &MockJs, build: F)
where
    F: FnOnce(async_nats::HeaderMap) -> (async_nats::HeaderMap, bytes::Bytes) + Send + 'static,
{
    let (consumer, tx) = trogon_nats::jetstream::MockJetStreamConsumer::new();
    js.consumer_factory.add_consumer(consumer);
    reply_when_published(&js.publisher, tx, build);
}

/// Sends a reply on `tx` once a request carrying a `Jsonrpc-Id` has been
/// published, giving the builder that request's headers.
///
/// For tests that need to wire up their consumers themselves.
pub fn reply_when_published<F>(
    publisher: &trogon_nats::jetstream::MockJetStreamPublisher,
    tx: futures::channel::mpsc::UnboundedSender<
        Result<trogon_nats::jetstream::MockJsMessage, trogon_nats::mocks::MockError>,
    >,
    build: F,
) where
    F: FnOnce(async_nats::HeaderMap) -> (async_nats::HeaderMap, bytes::Bytes) + Send + 'static,
{
    let publisher = publisher.clone();
    tokio::spawn(async move {
        // Yield before looking: this task is spawned before the caller
        // publishes, so the request is not on the publisher yet.
        let mut request = None;
        while request.is_none() {
            tokio::task::yield_now().await;
            request = publisher
                .published_messages()
                .into_iter()
                .find(|m| m.headers.get(jsonrpc_nats::HEADER_ID).is_some());
        }
        let request_headers = request.map(|m| m.headers).unwrap_or_default();

        let (headers, payload) = build(request_headers);
        let _ = tx.unbounded_send(Ok(trogon_nats::jetstream::MockJsMessage::new(async_nats::Message {
            subject: "test".into(),
            reply: None,
            payload,
            headers: Some(headers),
            status: None,
            description: None,
            length: 0,
        })));
    });
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

pub fn has_session_ready_error_metric(finished_metrics: &[opentelemetry_sdk::metrics::data::ResourceMetrics]) -> bool {
    has_error_metric(finished_metrics, "session_ready", "session_ready_publish_failed")
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
mod tests;
