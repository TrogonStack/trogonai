use super::region_id::RegionId;

/// Caller-supplied sink for multi-region routing audit events (kept separate from `crate::audit`).
pub trait RegionAuditSink: Send + Sync {
    fn region_failed_over(&self, from: &RegionId, to: &RegionId, reason: &str);
    fn region_recovered(&self, region: &RegionId);
}

#[derive(Debug, Default)]
pub struct NoopRegionAuditSink;

impl RegionAuditSink for NoopRegionAuditSink {
    fn region_failed_over(&self, _from: &RegionId, _to: &RegionId, _reason: &str) {}

    fn region_recovered(&self, _region: &RegionId) {}
}

/// Test harness that records audit callbacks.
#[derive(Debug, Default)]
pub struct RecordingRegionAuditSink {
    pub failed_overs: std::sync::Mutex<Vec<(RegionId, RegionId, String)>>,
    pub recoveries: std::sync::Mutex<Vec<RegionId>>,
}

impl RegionAuditSink for RecordingRegionAuditSink {
    fn region_failed_over(&self, from: &RegionId, to: &RegionId, reason: &str) {
        self.failed_overs
            .lock()
            .expect("audit lock")
            .push((from.clone(), to.clone(), reason.to_string()));
    }

    fn region_recovered(&self, region: &RegionId) {
        self.recoveries
            .lock()
            .expect("audit lock")
            .push(region.clone());
    }
}
