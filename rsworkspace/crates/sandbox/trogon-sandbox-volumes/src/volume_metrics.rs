use prometheus::{HistogramVec, IntCounterVec, Registry};

use crate::VolumeMetricsError;
use crate::metrics_text::{MetricSpec, counter, duration_histogram, gather_text};

const BUCKET_MODE: &[&str] = &["bucket", "mode"];
const BYTES_IN: MetricSpec = MetricSpec {
    name: "trogon_sandbox_volumes_bytes_in_total",
    help: "Bytes downloaded from object storage per bucket and mode",
    labels: BUCKET_MODE,
};
const BYTES_OUT: MetricSpec = MetricSpec {
    name: "trogon_sandbox_volumes_bytes_out_total",
    help: "Bytes uploaded to object storage per bucket and mode",
    labels: BUCKET_MODE,
};
const PULL_DURATION: MetricSpec = MetricSpec {
    name: "trogon_sandbox_volumes_pull_duration_seconds",
    help: "Duration of prefix pull operations",
    labels: BUCKET_MODE,
};
const PUSH_DURATION: MetricSpec = MetricSpec {
    name: "trogon_sandbox_volumes_push_duration_seconds",
    help: "Duration of prefix push operations",
    labels: BUCKET_MODE,
};
const CACHE_HIT: MetricSpec = MetricSpec {
    name: "trogon_sandbox_volumes_cache_hit_total",
    help: "Clone cache hits served from object storage",
    labels: &["bucket"],
};

/// Prometheus collectors for pull, push, and clone-cache activity.
#[derive(Clone)]
pub struct VolumeMetrics {
    registry: Registry,
    pub bytes_in_total: IntCounterVec,
    pub bytes_out_total: IntCounterVec,
    pub pull_duration_seconds: HistogramVec,
    pub push_duration_seconds: HistogramVec,
    pub cache_hit_total: IntCounterVec,
}

impl VolumeMetrics {
    pub fn new() -> Result<Self, VolumeMetricsError> {
        let registry = Registry::new();
        let bytes_in_total = counter(&registry, &BYTES_IN)?;
        let bytes_out_total = counter(&registry, &BYTES_OUT)?;
        let pull_duration_seconds = duration_histogram(&registry, &PULL_DURATION)?;
        let push_duration_seconds = duration_histogram(&registry, &PUSH_DURATION)?;
        let cache_hit_total = counter(&registry, &CACHE_HIT)?;
        Ok(Self {
            registry,
            bytes_in_total,
            bytes_out_total,
            pull_duration_seconds,
            push_duration_seconds,
            cache_hit_total,
        })
    }

    pub fn gather_text(&self) -> Result<String, VolumeMetricsError> {
        gather_text(&self.registry)
    }
}
