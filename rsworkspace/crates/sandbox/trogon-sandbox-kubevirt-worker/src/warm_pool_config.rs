/// Warm pool sizing and eviction policy for pre-started VMs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WarmPoolConfig {
    pub enabled: bool,
    /// Most idle VMs kept per snapshot digest. VMs are expensive, so the default is conservative.
    pub max_pool_size_per_digest: u32,
    /// Cap on warm VMs across all digests on this worker.
    pub max_total_warm: u32,
    /// Idle warm VMs are deleted after this many seconds.
    pub max_idle_seconds: u64,
    /// Guest memory every warm VM boots with. A lease that asks for more cold starts, so the pool never hands
    /// out a VM smaller than requested.
    pub warm_memory_mib: u32,
}

impl Default for WarmPoolConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_pool_size_per_digest: 1,
            max_total_warm: 4,
            max_idle_seconds: 1800,
            warm_memory_mib: 512,
        }
    }
}

impl WarmPoolConfig {
    /// Whether a lease asking for `memory_mib` can run on a warm VM.
    pub fn fits(&self, memory_mib: u32) -> bool {
        self.enabled && memory_mib <= self.warm_memory_mib
    }
}
