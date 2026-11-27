use serde::{Deserialize, Serialize};

use crate::constants::{DEFAULT_SNAPSHOT_BYTES_CAP, DEFAULT_SNAPSHOT_COUNT_CAP};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotQuota {
    pub count_used: u64,
    pub count_cap: u64,
    pub bytes_used: u64,
    pub bytes_cap: u64,
}

impl Default for SnapshotQuota {
    fn default() -> Self {
        Self::new(DEFAULT_SNAPSHOT_COUNT_CAP, DEFAULT_SNAPSHOT_BYTES_CAP)
    }
}

impl SnapshotQuota {
    pub fn new(count_cap: u64, bytes_cap: u64) -> Self {
        Self {
            count_used: 0,
            count_cap,
            bytes_used: 0,
            bytes_cap,
        }
    }

    pub fn would_exceed_publish(&self, bytes: u64) -> bool {
        self.count_used
            .checked_add(1)
            .is_none_or(|count| count > self.count_cap)
            || self
                .bytes_used
                .checked_add(bytes)
                .is_none_or(|total| total > self.bytes_cap)
    }

    pub fn record_publish(&mut self, bytes: u64) {
        self.count_used = self.count_used.saturating_add(1);
        self.bytes_used = self.bytes_used.saturating_add(bytes);
    }

    pub fn record_delete(&mut self, bytes: u64) {
        self.count_used = self.count_used.saturating_sub(1);
        self.bytes_used = self.bytes_used.saturating_sub(bytes);
    }
}
