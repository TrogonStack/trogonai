//! Pool and engine configuration (ADR 0025 defaults).

/// Tunables for component pooling per bundle digest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PoolConfig {
    /// Instances created at bundle activation.
    pub prewarm: u32,
    /// Maximum idle+in-flight instances per `(digest, component_id)`.
    pub max_instances: u32,
    /// Milliseconds to wait for an idle instance before `rate_limited`.
    pub acquire_timeout_ms: u64,
    /// Fuel budget per `evaluate` checkout.
    pub fuel_evaluate: u64,
    /// Fuel budget for guest `init` during instance creation.
    pub fuel_init: u64,
    /// Maximum host import calls per evaluation.
    pub max_host_imports: u32,
    /// Linear memory cap per instance (bytes).
    pub max_memory_bytes: usize,
}

impl Default for PoolConfig {
    fn default() -> Self {
        Self {
            prewarm: 4,
            max_instances: 32,
            acquire_timeout_ms: 50,
            fuel_evaluate: 5_000_000,
            fuel_init: 500_000,
            max_host_imports: 64,
            max_memory_bytes: 16 * 1024 * 1024,
        }
    }
}

impl PoolConfig {
    #[must_use]
    pub fn for_tests() -> Self {
        Self {
            prewarm: 2,
            max_instances: 4,
            acquire_timeout_ms: 200,
            fuel_evaluate: 5_000_000,
            fuel_init: 500_000,
            max_host_imports: 64,
            max_memory_bytes: 16 * 1024 * 1024,
        }
    }
}
