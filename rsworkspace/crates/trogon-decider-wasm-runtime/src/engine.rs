use wasmtime::{Engine, Store, StoreLimits, StoreLimitsBuilder};

/// Default fuel budget for a single guest export call (`decide`, `evolve`, or
/// `snapshot`). Wasmtime decrements fuel per instruction executed, so a guest
/// stuck in an infinite loop traps with `OutOfFuel` instead of blocking the
/// host indefinitely.
pub const DEFAULT_FUEL_PER_CALL: u64 = 10_000_000;

/// Default ceiling on a single store's linear-memory growth. Bounds how much
/// memory one guest session can pin regardless of how many events it replays.
pub const DEFAULT_MAX_MEMORY_BYTES: usize = 64 * 1024 * 1024;

/// Tunable resource limits applied to every guest session created from a
/// [`WasmDeciderEngine`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WasmEngineConfig {
    max_memory_bytes: usize,
    fuel_per_call: u64,
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
}

impl Default for WasmEngineConfig {
    fn default() -> Self {
        Self {
            max_memory_bytes: DEFAULT_MAX_MEMORY_BYTES,
            fuel_per_call: DEFAULT_FUEL_PER_CALL,
        }
    }
}

/// Per-session store state. Wraps [`StoreLimits`] so wasmtime enforces the
/// memory ceiling on `memory.grow` without the host having to poll it.
pub(crate) struct GuestState {
    pub(crate) limits: StoreLimits,
}

impl GuestState {
    fn new(config: &WasmEngineConfig) -> Self {
        Self {
            limits: StoreLimitsBuilder::new().memory_size(config.max_memory_bytes).build(),
        }
    }
}

/// Shared wasmtime [`Engine`] configured for the component model with fuel
/// metering enabled, plus the resource limits applied to every guest store
/// created from it.
///
/// One [`WasmDeciderEngine`] is meant to be built once per process and shared
/// across every loaded [`crate::WasmDeciderModule`].
#[derive(Clone)]
pub struct WasmDeciderEngine {
    engine: Engine,
    config: WasmEngineConfig,
}

impl WasmDeciderEngine {
    /// Builds a new engine with the given resource limits.
    pub fn new(config: WasmEngineConfig) -> Result<Self, wasmtime::Error> {
        let mut wasmtime_config = wasmtime::Config::new();
        wasmtime_config.wasm_component_model(true);
        wasmtime_config.consume_fuel(true);
        let engine = Engine::new(&wasmtime_config)?;
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

    /// Creates a fresh per-session store with the memory limiter installed, so
    /// wasmtime enforces the configured ceiling on `memory.grow`.
    pub(crate) fn new_store(&self) -> Store<GuestState> {
        let mut store = Store::new(&self.engine, GuestState::new(&self.config));
        store.limiter(|state| &mut state.limits);
        store
    }
}

#[cfg(test)]
mod tests;
