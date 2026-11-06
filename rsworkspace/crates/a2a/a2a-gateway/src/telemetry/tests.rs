use std::time::Duration;

use opentelemetry::metrics::MeterProvider;
use opentelemetry_sdk::metrics::data::{AggregatedMetrics, MetricData};
use opentelemetry_sdk::metrics::{InMemoryMetricExporter, PeriodicReader, SdkMeterProvider};

use super::RelayAdmissionMetrics;

#[test]
fn capacity_refusals_export_only_the_two_bounded_admission_scopes() {
    let exporter = InMemoryMetricExporter::default();
    let reader = PeriodicReader::builder(exporter.clone())
        .with_interval(Duration::from_secs(3600))
        .build();
    let provider = SdkMeterProvider::builder().with_reader(reader).build();
    let metrics = RelayAdmissionMetrics::new(&provider.meter("a2a-gateway"));
    metrics.global_refusal();
    metrics.caller_refusal();
    metrics.caller_refusal();
    provider.force_flush().expect("flush");
    let exported = exporter.get_finished_metrics().expect("metrics");
    let metric = exported
        .iter()
        .flat_map(|resource| resource.scope_metrics())
        .flat_map(|scope| scope.metrics())
        .find(|metric| metric.name() == trogon_semconv::metric::A2A_GATEWAY_IMPORTED_REPLY_CAPACITY_REFUSALS)
        .expect("refusal counter");
    let AggregatedMetrics::U64(MetricData::Sum(sum)) = metric.data() else {
        panic!("counter type");
    };
    let mut points = sum
        .data_points()
        .map(|point| {
            let attributes: Vec<_> = point.attributes().collect();
            assert_eq!(attributes.len(), 1);
            assert_eq!(
                attributes[0].key.as_str(),
                trogon_semconv::attribute::A2A_GATEWAY_RELAY_LIMIT
            );
            (attributes[0].value.to_string(), point.value())
        })
        .collect::<Vec<_>>();
    points.sort();
    assert_eq!(points, vec![("caller".into(), 2), ("global".into(), 1)]);
}
