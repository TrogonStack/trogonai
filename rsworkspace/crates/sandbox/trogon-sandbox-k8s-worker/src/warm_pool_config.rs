/// Bounds and eviction policy for the warm Pod pool.
#[derive(Debug, Clone, Copy)]
pub struct WarmPoolConfig {
    pub max_pool_size_per_digest: u32,
    pub max_total_warm: u32,
    pub max_idle_seconds: u64,
    /// Most warm pods one reconcile pass creates, so a restart or a burst of
    /// claims refills the pool gradually instead of in one spike.
    pub refill_per_pass: u32,
    /// Most warm pods allowed to be starting (created, not yet running) at
    /// once, across all digests.
    pub max_warming: u32,
}

impl Default for WarmPoolConfig {
    fn default() -> Self {
        Self {
            max_pool_size_per_digest: 3,
            max_total_warm: 16,
            max_idle_seconds: 600,
            refill_per_pass: 2,
            max_warming: 4,
        }
    }
}

#[cfg(test)]
mod tests;
