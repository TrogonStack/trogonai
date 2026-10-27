use std::time::Duration;

use axum::{
    Router,
    body::{Body, to_bytes},
    extract::Request,
    response::Response,
};
use opentelemetry::{KeyValue, metrics::MeterProvider, trace::TracerProvider};
use opentelemetry_sdk::{
    metrics::{
        InMemoryMetricExporter, PeriodicReader, SdkMeterProvider,
        data::{AggregatedMetrics, MetricData},
    },
    trace::{InMemorySpanExporter, Sampler, SdkTracerProvider, SpanData},
};
use tower::ServiceExt;
use tracing::instrument::WithSubscriber;
use tracing_subscriber::prelude::*;

use super::super::HttpTelemetry;

pub(super) struct DurationPoint {
    pub(super) count: u64,
    pub(super) attributes: Vec<KeyValue>,
    pub(super) bounds: Vec<f64>,
    pub(super) unit: String,
}

pub(super) struct TestTelemetry {
    telemetry: HttpTelemetry,
    subscriber: tracing::Dispatch,
    traces: SdkTracerProvider,
    spans: InMemorySpanExporter,
    meters: SdkMeterProvider,
    metrics: InMemoryMetricExporter,
}

impl TestTelemetry {
    pub(super) fn new() -> Self {
        let spans = InMemorySpanExporter::default();
        let traces = SdkTracerProvider::builder()
            .with_simple_exporter(spans.clone())
            .with_sampler(Sampler::ParentBased(Box::new(Sampler::AlwaysOn)))
            .build();
        let subscriber = tracing::Dispatch::new(
            tracing_subscriber::registry()
                .with(tracing_opentelemetry::layer().with_tracer(traces.tracer("gateway-http-test"))),
        );
        let metrics = InMemoryMetricExporter::default();
        let reader = PeriodicReader::builder(metrics.clone())
            .with_interval(Duration::from_secs(3600))
            .build();
        let meters = SdkMeterProvider::builder().with_reader(reader).build();
        let telemetry = HttpTelemetry::new(meters.meter("gateway-http-test"));
        Self {
            telemetry,
            subscriber,
            traces,
            spans,
            meters,
            metrics,
        }
    }

    pub(super) async fn request(&self, router: Router, request: Request) -> Response {
        self.telemetry
            .instrument(router)
            .oneshot(request)
            .with_subscriber(self.subscriber.clone())
            .await
            .unwrap()
    }

    pub(super) async fn consume(&self, response: Response<Body>) {
        to_bytes(response.into_body(), usize::MAX)
            .with_subscriber(self.subscriber.clone())
            .await
            .unwrap();
    }

    pub(super) fn spans(&self) -> Vec<SpanData> {
        self.traces.force_flush().unwrap();
        self.spans.get_finished_spans().unwrap()
    }

    pub(super) fn duration_points(&self) -> Vec<DurationPoint> {
        self.metrics.reset();
        self.meters.force_flush().unwrap();
        let mut points = Vec::new();
        for resource in self.metrics.get_finished_metrics().unwrap() {
            for scope in resource.scope_metrics() {
                for metric in scope.metrics() {
                    assert_eq!(metric.name(), trogon_semconv::metric::HTTP_SERVER_REQUEST_DURATION);
                    if let AggregatedMetrics::F64(MetricData::Histogram(histogram)) = metric.data() {
                        points.extend(histogram.data_points().map(|point| DurationPoint {
                            count: point.count(),
                            attributes: point.attributes().cloned().collect(),
                            bounds: point.bounds().collect(),
                            unit: metric.unit().to_owned(),
                        }));
                    }
                }
            }
        }
        points
    }
}
