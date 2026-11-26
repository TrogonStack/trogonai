use prometheus::{HistogramVec, IntCounterVec, Registry};

use crate::VolumeMetricsError;
use crate::metrics_text::{MetricSpec, counter, duration_histogram, gather_text};

const MOUNT_TOTAL: MetricSpec = MetricSpec {
    name: "trogon_sandbox_volumes_fuse_mount_total",
    help: "Total FUSE mounts attempted per bucket",
    labels: &["bucket"],
};
const MOUNT_DURATION: MetricSpec = MetricSpec {
    name: "trogon_sandbox_volumes_fuse_mount_duration_seconds",
    help: "Duration of FUSE mount operations per bucket",
    labels: &["bucket"],
};

/// Prometheus collectors for FUSE mount activity.
#[derive(Clone)]
pub struct FuseMetrics {
    registry: Registry,
    pub fuse_mount_total: IntCounterVec,
    pub fuse_mount_duration_seconds: HistogramVec,
}

impl FuseMetrics {
    pub fn new() -> Result<Self, VolumeMetricsError> {
        let registry = Registry::new();
        let fuse_mount_total = counter(&registry, &MOUNT_TOTAL)?;
        let fuse_mount_duration_seconds = duration_histogram(&registry, &MOUNT_DURATION)?;
        Ok(Self {
            registry,
            fuse_mount_total,
            fuse_mount_duration_seconds,
        })
    }

    pub fn gather_text(&self) -> Result<String, VolumeMetricsError> {
        gather_text(&self.registry)
    }
}
