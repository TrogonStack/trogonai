use std::time::Duration;

use wasmtime::{
    Engine, EngineWeak, InstanceAllocationStrategy, PoolingAllocationConfig, Store, StoreLimits, StoreLimitsBuilder,
};

use crate::constants::{
    DEFAULT_EPOCH_TICK_INTERVAL, DEFAULT_EPOCH_TICKS_PER_CALL, DEFAULT_FUEL_PER_CALL, DEFAULT_MAX_CONCURRENT_SESSIONS,
    DEFAULT_MAX_INSTANCES_PER_SESSION, DEFAULT_MAX_MEMORIES_PER_SESSION, DEFAULT_MAX_MEMORY_BYTES,
    DEFAULT_MAX_TABLE_ELEMENTS, DEFAULT_MAX_TABLES_PER_SESSION,
};

/// Tunable resource limits applied to every guest session created from a
/// [`WasmDeciderEngine`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WasmEngineConfig {
    max_memory_bytes: usize,
    fuel_per_call: u64,
    max_table_elements: usize,
    max_instances_per_session: usize,
    max_tables_per_session: usize,
    max_memories_per_session: usize,
    max_concurrent_sessions: usize,
    epoch_tick_interval: Duration,
    epoch_ticks_per_call: u64,
}

impl WasmEngineConfig {
    /// Returns the memory ceiling, in bytes, applied to each guest store.
    pub fn max_memory_bytes(&self) -> usize {
        self.max_memory_bytes
    }

    /// Returns the fuel budget applied before each guest export call.
    pub fn fuel_per_call(&self) -> u64 {
        self.fuel_per_call
    }

    /// Returns the table element ceiling applied to each guest store.
    pub fn max_table_elements(&self) -> usize {
        self.max_table_elements
    }

    /// Returns the core module instantiation ceiling applied to each guest
    /// store.
    pub fn max_instances_per_session(&self) -> usize {
        self.max_instances_per_session
    }

    /// Returns the table ceiling applied to each guest store.
    pub fn max_tables_per_session(&self) -> usize {
        self.max_tables_per_session
    }

    /// Returns the linear memory ceiling applied to each guest store.
    pub fn max_memories_per_session(&self) -> usize {
        self.max_memories_per_session
    }

    /// Returns how many guest sessions the pooling allocator keeps warm
    /// allocation slots for concurrently.
    pub fn max_concurrent_sessions(&self) -> usize {
        self.max_concurrent_sessions
    }

    /// Returns the cadence at which the background ticker increments the
    /// wasmtime epoch.
    pub fn epoch_tick_interval(&self) -> Duration {
        self.epoch_tick_interval
    }

    /// Returns the epoch ticks allowed for a single guest export call before
    /// it is interrupted.
    pub fn epoch_ticks_per_call(&self) -> u64 {
        self.epoch_ticks_per_call
    }

    /// Overrides the memory ceiling applied to each guest store.
    pub fn with_max_memory_bytes(mut self, max_memory_bytes: usize) -> Self {
        self.max_memory_bytes = max_memory_bytes;
        self
    }

    /// Overrides the fuel budget applied before each guest export call.
    pub fn with_fuel_per_call(mut self, fuel_per_call: u64) -> Self {
        self.fuel_per_call = fuel_per_call;
        self
    }

    /// Overrides the table element ceiling applied to each guest store.
    pub fn with_max_table_elements(mut self, max_table_elements: usize) -> Self {
        self.max_table_elements = max_table_elements;
        self
    }

    /// Overrides the core module instantiation ceiling applied to each guest
    /// store.
    pub fn with_max_instances_per_session(mut self, max_instances_per_session: usize) -> Self {
        self.max_instances_per_session = max_instances_per_session;
        self
    }

    /// Overrides the table ceiling applied to each guest store.
    pub fn with_max_tables_per_session(mut self, max_tables_per_session: usize) -> Self {
        self.max_tables_per_session = max_tables_per_session;
        self
    }

    /// Overrides the linear memory ceiling applied to each guest store.
    pub fn with_max_memories_per_session(mut self, max_memories_per_session: usize) -> Self {
        self.max_memories_per_session = max_memories_per_session;
        self
    }

    /// Overrides how many guest sessions the pooling allocator keeps warm
    /// allocation slots for concurrently.
    pub fn with_max_concurrent_sessions(mut self, max_concurrent_sessions: usize) -> Self {
        self.max_concurrent_sessions = max_concurrent_sessions;
        self
    }

    /// Overrides the cadence at which the background ticker increments the
    /// wasmtime epoch.
    pub fn with_epoch_tick_interval(mut self, epoch_tick_interval: Duration) -> Self {
        self.epoch_tick_interval = epoch_tick_interval;
        self
    }

    /// Overrides the epoch ticks allowed for a single guest export call
    /// before it is interrupted.
    pub fn with_epoch_ticks_per_call(mut self, epoch_ticks_per_call: u64) -> Self {
        self.epoch_ticks_per_call = epoch_ticks_per_call;
        self
    }
}

impl Default for WasmEngineConfig {
    fn default() -> Self {
        Self {
            max_memory_bytes: DEFAULT_MAX_MEMORY_BYTES,
            fuel_per_call: DEFAULT_FUEL_PER_CALL,
            max_table_elements: DEFAULT_MAX_TABLE_ELEMENTS,
            max_instances_per_session: DEFAULT_MAX_INSTANCES_PER_SESSION,
            max_tables_per_session: DEFAULT_MAX_TABLES_PER_SESSION,
            max_memories_per_session: DEFAULT_MAX_MEMORIES_PER_SESSION,
            max_concurrent_sessions: DEFAULT_MAX_CONCURRENT_SESSIONS,
            epoch_tick_interval: DEFAULT_EPOCH_TICK_INTERVAL,
            epoch_ticks_per_call: DEFAULT_EPOCH_TICKS_PER_CALL,
        }
    }
}

/// Per-session store state. Wraps [`StoreLimits`] so wasmtime enforces the
/// resource ceilings on growth (memory, table elements, instance, table, and
/// memory counts) without the host having to poll them.
pub(crate) struct GuestState {
    pub(crate) limits: StoreLimits,
}

impl GuestState {
    fn new(config: &WasmEngineConfig) -> Self {
        Self {
            limits: StoreLimitsBuilder::new()
                .memory_size(config.max_memory_bytes)
                .table_elements(config.max_table_elements)
                .instances(config.max_instances_per_session)
                .tables(config.max_tables_per_session)
                .memories(config.max_memories_per_session)
                .build(),
        }
    }
}

/// Shared wasmtime [`Engine`] configured for the component model with fuel
/// metering and epoch-based interruption enabled, the pooling allocator
/// installed, plus the resource limits applied to every guest store created
/// from it.
///
/// One [`WasmDeciderEngine`] is meant to be built once per process and shared
/// across every loaded [`crate::WasmDeciderModule`]. Building one also starts
/// a background thread that ticks the engine's epoch on a fixed cadence; the
/// thread holds only a weak reference to the underlying wasmtime [`Engine`]
/// and exits on its own once every clone of it has been dropped.
#[derive(Clone)]
pub struct WasmDeciderEngine {
    engine: Engine,
    config: WasmEngineConfig,
}

/// Failure building the shared wasmtime engine.
#[derive(Debug, thiserror::Error)]
pub enum WasmEngineError {
    /// Wasmtime rejected the configuration.
    #[error("failed to configure wasmtime engine")]
    Configure(#[source] wasmtime::Error),
    /// The background epoch ticker thread could not be started.
    #[error("failed to start the epoch interruption ticker thread")]
    EpochTicker(#[source] std::io::Error),
}

impl WasmDeciderEngine {
    /// Builds a new engine with the given resource limits.
    pub fn new(config: WasmEngineConfig) -> Result<Self, WasmEngineError> {
        let mut wasmtime_config = wasmtime::Config::new();
        wasmtime_config.wasm_component_model(true);
        wasmtime_config.consume_fuel(true);
        wasmtime_config.epoch_interruption(true);
        wasmtime_config.allocation_strategy(InstanceAllocationStrategy::Pooling(pooling_allocation_config(&config)));
        let engine = Engine::new(&wasmtime_config).map_err(WasmEngineError::Configure)?;
        spawn_epoch_ticker(engine.weak(), config.epoch_tick_interval).map_err(WasmEngineError::EpochTicker)?;
        Ok(Self { engine, config })
    }

    /// Returns the underlying wasmtime [`Engine`].
    pub fn engine(&self) -> &Engine {
        &self.engine
    }

    /// Returns the resource limits applied to every guest store.
    pub fn config(&self) -> WasmEngineConfig {
        self.config
    }

    /// Creates a fresh per-session store with the resource limiter installed,
    /// so wasmtime enforces the configured ceilings on memory, table, and
    /// instance growth.
    pub(crate) fn new_store(&self) -> Store<GuestState> {
        let mut store = Store::new(&self.engine, GuestState::new(&self.config));
        store.limiter(|state| &mut state.limits);
        store
    }

    /// Arms a guest export call with both a fuel budget and a wall-clock
    /// epoch deadline.
    ///
    /// Call this immediately before every guest export call so a
    /// fuel-cheap-but-slow guest still traps within a bounded amount of wall
    /// clock time, not only a bounded instruction count.
    pub(crate) fn arm_guest_call<T>(&self, store: &mut Store<T>, fuel: u64, epoch_ticks: u64) -> wasmtime::Result<()> {
        store.set_fuel(fuel)?;
        store.set_epoch_deadline(epoch_ticks);
        Ok(())
    }
}

/// Sizes the pooling allocator consistently with the per-session resource
/// limits: the per-component maximums match the per-store [`StoreLimits`]
/// installed by [`GuestState::new`], and the pool-wide totals are that many
/// resources times [`WasmEngineConfig::max_concurrent_sessions`].
fn pooling_allocation_config(config: &WasmEngineConfig) -> PoolingAllocationConfig {
    let sessions = saturating_u32(config.max_concurrent_sessions);
    let instances_per_session = saturating_u32(config.max_instances_per_session);
    let tables_per_session = saturating_u32(config.max_tables_per_session);
    let memories_per_session = saturating_u32(config.max_memories_per_session);

    let mut pooling = PoolingAllocationConfig::new();
    pooling
        .total_component_instances(sessions)
        .total_core_instances(sessions.saturating_mul(instances_per_session))
        .total_tables(sessions.saturating_mul(tables_per_session))
        .total_memories(sessions.saturating_mul(memories_per_session))
        .max_core_instances_per_component(instances_per_session)
        .max_tables_per_component(tables_per_session)
        .max_memories_per_component(memories_per_session)
        .table_elements(config.max_table_elements)
        .max_memory_size(config.max_memory_bytes);
    pooling
}

fn saturating_u32(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

/// Starts the background thread that periodically increments the wasmtime
/// engine's epoch, driving epoch-based interruption.
///
/// The thread only holds an [`EngineWeak`], so it never keeps the engine
/// alive on its own; once every [`WasmDeciderEngine`] (and therefore every
/// clone of the underlying [`Engine`]) is dropped, the next tick observes a
/// failed upgrade and the thread exits.
fn spawn_epoch_ticker(engine: EngineWeak, tick_interval: Duration) -> std::io::Result<()> {
    std::thread::Builder::new()
        .name("wasm-decider-epoch-ticker".to_string())
        .spawn(move || {
            loop {
                std::thread::sleep(tick_interval);
                match engine.upgrade() {
                    Some(engine) => engine.increment_epoch(),
                    None => break,
                }
            }
        })
        .map(|_join_handle| ())
}

#[cfg(test)]
mod tests;
