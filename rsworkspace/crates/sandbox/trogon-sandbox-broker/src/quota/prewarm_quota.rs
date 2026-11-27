use serde::{Deserialize, Serialize};

use crate::constants::{DEFAULT_PREWARM_COUNT_CAP, DEFAULT_PREWARM_CPU_CAP};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrewarmQuota {
    pub active_count: u32,
    pub count_cap: u32,
    pub total_cpu_seconds: u64,
    pub cpu_cap: u64,
}

impl Default for PrewarmQuota {
    fn default() -> Self {
        Self {
            active_count: 0,
            count_cap: DEFAULT_PREWARM_COUNT_CAP,
            total_cpu_seconds: 0,
            cpu_cap: DEFAULT_PREWARM_CPU_CAP,
        }
    }
}

impl PrewarmQuota {
    pub fn would_exceed_dispatch(&self, cpu_seconds: u32) -> bool {
        self.active_count.saturating_add(1) > self.count_cap
            || self.total_cpu_seconds.saturating_add(u64::from(cpu_seconds)) > self.cpu_cap
    }

    pub fn record_dispatch(&mut self, cpu_seconds: u32) {
        self.active_count = self.active_count.saturating_add(1);
        self.total_cpu_seconds = self.total_cpu_seconds.saturating_add(u64::from(cpu_seconds));
    }

    pub fn record_release(&mut self, cpu_seconds: u32) {
        self.active_count = self.active_count.saturating_sub(1);
        self.total_cpu_seconds = self.total_cpu_seconds.saturating_sub(u64::from(cpu_seconds));
    }
}
