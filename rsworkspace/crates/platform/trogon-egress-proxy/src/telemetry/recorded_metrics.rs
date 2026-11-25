//! A meter whose counters the crate's unit tests read back.

use std::time::Duration;

use opentelemetry::metrics::{Meter, MeterProvider};
use opentelemetry_sdk::metrics::data::{AggregatedMetrics, MetricData};
use opentelemetry_sdk::metrics::{InMemoryMetricExporter, PeriodicReader, SdkMeterProvider};

use crate::constants::METER_NAME;

/// Exports only when read, so a test sees exactly what was recorded so far.
pub(crate) struct RecordedMetrics {
    provider: SdkMeterProvider,
    exporter: InMemoryMetricExporter,
}

impl RecordedMetrics {
    pub(crate) fn new() -> Self {
        let exporter = InMemoryMetricExporter::default();
        let reader = PeriodicReader::builder(exporter.clone())
            .with_interval(Duration::from_secs(3_600))
            .build();
        Self {
            provider: SdkMeterProvider::builder().with_reader(reader).build(),
            exporter,
        }
    }

    pub(crate) fn meter(&self) -> Meter {
        self.provider.meter(METER_NAME)
    }

    /// What the counter `name` counted so far, one entry per set of
    /// attributes, each attribute as `key=value`, sorted.
    pub(crate) fn counts(&self, name: &str) -> Vec<(Vec<String>, u64)> {
        self.exporter.reset();
        self.provider.force_flush().expect("flush");
        let exported = self.exporter.get_finished_metrics().expect("metrics");
        let mut counts: Vec<_> = exported
            .iter()
            .flat_map(|resource| resource.scope_metrics())
            .flat_map(|scope| scope.metrics())
            .filter(|metric| metric.name() == name)
            .flat_map(|metric| {
                let AggregatedMetrics::U64(MetricData::Sum(sum)) = metric.data() else {
                    panic!("{name} is not a counter of whole numbers");
                };
                sum.data_points()
                    .map(|point| {
                        let attributes = point
                            .attributes()
                            .map(|attribute| format!("{}={}", attribute.key.as_str(), attribute.value))
                            .collect();
                        (attributes, point.value())
                    })
                    .collect::<Vec<_>>()
            })
            .collect();
        counts.sort();
        counts
    }
}

#[cfg(test)]
mod tests;
