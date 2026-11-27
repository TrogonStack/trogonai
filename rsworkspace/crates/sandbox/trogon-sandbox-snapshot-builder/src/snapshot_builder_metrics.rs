use prometheus::{IntCounterVec, IntGauge, Opts, Registry};

/// The counters and gauges the builder reports, registered on a shared registry.
#[derive(Clone)]
pub struct SnapshotBuilderMetrics {
    pub pushed_total: IntCounterVec,
    pub failed_total: IntCounterVec,
    pub inflight: IntGauge,
    pub queue_depth: IntGauge,
}

impl SnapshotBuilderMetrics {
    pub fn register(registry: &Registry) -> Result<Self, prometheus::Error> {
        let pushed_total = IntCounterVec::new(
            Opts::new(
                "snapshot_builder_pushed_total",
                "Snapshots recorded after worker export",
            ),
            &["artifact_type"],
        )?;
        registry.register(Box::new(pushed_total.clone()))?;

        let failed_total = IntCounterVec::new(
            Opts::new("snapshot_builder_failed_total", "Snapshot builder failures by reason"),
            &["reason"],
        )?;
        registry.register(Box::new(failed_total.clone()))?;

        let inflight = IntGauge::new("snapshot_builder_inflight", "Concurrent worker export RPCs in flight")?;
        registry.register(Box::new(inflight.clone()))?;

        let queue_depth = IntGauge::new(
            "snapshot_builder_queue_depth",
            "Pending snapshot export jobs waiting for a worker slot",
        )?;
        registry.register(Box::new(queue_depth.clone()))?;

        Ok(Self {
            pushed_total,
            failed_total,
            inflight,
            queue_depth,
        })
    }
}
