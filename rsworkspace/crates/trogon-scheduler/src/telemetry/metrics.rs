//! Outcome and operation metrics for the reconciliation consumer.
//!
//! Reconciliation is a side-effecting consumer, so observability is part of the
//! component design. Metric names follow the dotted `otel-name-metric`
//! convention. Datadog dashboard wiring follows from these names but is out of
//! the component slice.

use opentelemetry::metrics::Counter;
use opentelemetry::{KeyValue, global};
use trogon_semconv::{attribute, metric};

const METER_NAME: &str = "trogon-scheduler";

/// Counters recorded for each processed record and execution operation.
#[derive(Clone)]
pub struct ProcessorMetrics {
    records: Counter<u64>,
    publishes: Counter<u64>,
    purges: Counter<u64>,
    redeliveries: Counter<u64>,
}

impl ProcessorMetrics {
    /// Builds the processor counters against the global meter.
    pub fn new() -> Self {
        let meter = global::meter(METER_NAME);
        Self {
            records: metric::build_scheduler_processor_records(&meter),
            publishes: metric::build_scheduler_processor_execution_publishes(&meter),
            purges: metric::build_scheduler_processor_execution_purges(&meter),
            redeliveries: metric::build_scheduler_processor_redeliveries(&meter),
        }
    }

    /// Records one processed record with its outcome label.
    pub fn record_outcome(&self, outcome: &'static str) {
        self.records.add(1, &[KeyValue::new(attribute::OUTCOME, outcome)]);
    }

    /// Records one execution schedule publish.
    pub fn record_publish(&self) {
        self.publishes.add(1, &[]);
    }

    /// Records one execution schedule purge.
    pub fn record_purge(&self) {
        self.purges.add(1, &[]);
    }

    /// Records that a processed record was a NATS redelivery.
    pub fn record_redelivery(&self) {
        self.redeliveries.add(1, &[]);
    }
}

impl Default for ProcessorMetrics {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for ProcessorMetrics {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("ProcessorMetrics").finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests;
