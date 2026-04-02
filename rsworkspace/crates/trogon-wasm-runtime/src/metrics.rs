use std::sync::atomic::{AtomicU64, Ordering};

/// Global runtime metrics. All counters are monotonically increasing.
pub struct Metrics {
    /// Total WASM terminals started.
    pub wasm_tasks_started: AtomicU64,
    /// Total WASM terminals that completed successfully (exit_code set).
    pub wasm_tasks_completed: AtomicU64,
    /// Total WASM terminals that exited with a signal/trap.
    pub wasm_tasks_faulted: AtomicU64,
    /// Total fuel_exhausted signals.
    pub wasm_fuel_exhausted: AtomicU64,
    /// Total native process terminals started.
    pub native_tasks_started: AtomicU64,
    /// Module cache hits (in-memory or on-disk).
    pub cache_hits: AtomicU64,
    /// Module cache misses (compile needed).
    pub cache_misses: AtomicU64,
    /// Total host function calls (all trogon.* functions).
    pub host_calls_total: AtomicU64,
}

impl Metrics {
    pub const fn new() -> Self {
        Self {
            wasm_tasks_started: AtomicU64::new(0),
            wasm_tasks_completed: AtomicU64::new(0),
            wasm_tasks_faulted: AtomicU64::new(0),
            wasm_fuel_exhausted: AtomicU64::new(0),
            native_tasks_started: AtomicU64::new(0),
            cache_hits: AtomicU64::new(0),
            cache_misses: AtomicU64::new(0),
            host_calls_total: AtomicU64::new(0),
        }
    }

    pub fn snapshot(&self) -> MetricsSnapshot {
        MetricsSnapshot {
            wasm_tasks_started: self.wasm_tasks_started.load(Ordering::Relaxed),
            wasm_tasks_completed: self.wasm_tasks_completed.load(Ordering::Relaxed),
            wasm_tasks_faulted: self.wasm_tasks_faulted.load(Ordering::Relaxed),
            wasm_fuel_exhausted: self.wasm_fuel_exhausted.load(Ordering::Relaxed),
            native_tasks_started: self.native_tasks_started.load(Ordering::Relaxed),
            cache_hits: self.cache_hits.load(Ordering::Relaxed),
            cache_misses: self.cache_misses.load(Ordering::Relaxed),
            host_calls_total: self.host_calls_total.load(Ordering::Relaxed),
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct MetricsSnapshot {
    pub wasm_tasks_started: u64,
    pub wasm_tasks_completed: u64,
    pub wasm_tasks_faulted: u64,
    pub wasm_fuel_exhausted: u64,
    pub native_tasks_started: u64,
    pub cache_hits: u64,
    pub cache_misses: u64,
    pub host_calls_total: u64,
}

/// Global singleton metrics instance.
///
/// All counters are monotonically increasing for the lifetime of the process.
/// Tests that assert on metric values must use the delta pattern — snapshot
/// before and after, then assert the difference — to avoid coupling to
/// whatever previous tests have incremented:
///
/// ```rust
/// let before = METRICS.wasm_tasks_started.load(Ordering::Relaxed);
/// // … exercise the code …
/// let after = METRICS.wasm_tasks_started.load(Ordering::Relaxed);
/// assert_eq!(after - before, 1);
/// ```
pub static METRICS: Metrics = Metrics::new();
