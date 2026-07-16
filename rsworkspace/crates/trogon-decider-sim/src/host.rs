use thiserror::Error;
use trogon_decider_wasm_runtime::{WasmDeciderEngine, WasmEngineConfig, WasmEngineError};
use trogon_decider_wit::host::{self, Decider};
use wasmtime::component::{Component, Linker};
use wasmtime::{Store, StoreLimits, StoreLimitsBuilder};

use crate::import_check::{ImportCheckError, assert_zero_imports};

#[derive(Debug, Error)]
pub enum SimError {
    #[error(transparent)]
    Import(#[from] ImportCheckError),
    #[error("failed to configure wasmtime engine")]
    Engine(#[from] WasmEngineError),
    #[error("failed to compile wasm component")]
    Compile {
        #[source]
        source: wasmtime::Error,
    },
    #[error("failed to instantiate wasm component")]
    Instantiate {
        #[source]
        source: wasmtime::Error,
    },
    #[error("failed to arm guest call resource budget")]
    Arm {
        #[source]
        source: wasmtime::Error,
    },
}

/// Loaded decider component ready to instantiate in-memory test sessions.
///
/// Every guest call runs against the same resource budget
/// [`trogon_decider_wasm_runtime::WasmDeciderEngine`] applies in production:
/// fuel metering, epoch-based wall-clock interruption, and pooled
/// memory/table/instance ceilings. A decider that would trap in production
/// traps the same way here, instead of running unbounded in sim and CI.
pub struct SimHost {
    engine: WasmDeciderEngine,
    component: Component,
}

impl SimHost {
    /// Loads a decider component with the default production resource budget.
    pub fn load(bytes: &[u8]) -> Result<Self, SimError> {
        Self::load_with_config(bytes, WasmEngineConfig::default())
    }

    /// Loads a decider component with an explicit resource budget, letting
    /// tests exercise trapping behavior (fuel exhaustion, epoch interruption,
    /// memory ceilings) deterministically.
    pub fn load_with_config(bytes: &[u8], config: WasmEngineConfig) -> Result<Self, SimError> {
        assert_zero_imports(bytes)?;

        let engine = WasmDeciderEngine::new(config)?;
        let component = Component::new(engine.engine(), bytes).map_err(|source| SimError::Compile { source })?;

        Ok(Self { engine, component })
    }

    /// Returns the resource budget applied to every guest store created from
    /// this host.
    pub fn config(&self) -> WasmEngineConfig {
        self.engine.config()
    }

    pub fn instantiate<T: 'static>(&self, host_state: T) -> Result<SimInstance<T>, SimError> {
        let config = self.engine.config();
        let linker = Linker::new(self.engine.engine());
        let mut store = Store::new(self.engine.engine(), SimGuestState::new(&config, host_state));
        store.limiter(|state| &mut state.limits);
        arm_guest_call(&mut store, &config)?;
        let bindings = Decider::instantiate(&mut store, &self.component, &linker)
            .map_err(|source| SimError::Instantiate { source })?;
        Ok(SimInstance {
            store,
            bindings,
            config,
        })
    }
}

/// Per-session store state: the resource limiter production installs plus the
/// caller-supplied host state.
pub(crate) struct SimGuestState<T> {
    pub(crate) limits: StoreLimits,
    #[allow(
        dead_code,
        reason = "kept as the store's data so future host functions can read it via store.data()"
    )]
    pub(crate) host_state: T,
}

impl<T> SimGuestState<T> {
    fn new(config: &WasmEngineConfig, host_state: T) -> Self {
        Self {
            limits: StoreLimitsBuilder::new()
                .memory_size(config.max_memory_bytes())
                .table_elements(config.max_table_elements())
                .instances(config.max_instances_per_session())
                .tables(config.max_tables_per_session())
                .memories(config.max_memories_per_session())
                .build(),
            host_state,
        }
    }
}

/// Arms a guest export call with the same fuel budget and wall-clock epoch
/// deadline production sets immediately before every guest export call.
pub(crate) fn arm_guest_call<T>(store: &mut Store<T>, config: &WasmEngineConfig) -> Result<(), SimError> {
    store
        .set_fuel(config.fuel_per_call())
        .map_err(|source| SimError::Arm { source })?;
    store.set_epoch_deadline(config.epoch_ticks_per_call());
    Ok(())
}

pub struct SimInstance<T: 'static> {
    pub(crate) store: Store<SimGuestState<T>>,
    pub(crate) bindings: Decider,
    pub(crate) config: WasmEngineConfig,
}

impl<T: 'static> SimInstance<T> {
    pub fn descriptor(&mut self) -> Result<host::ModuleDescriptor, wasmtime::Error> {
        arm_guest_call(&mut self.store, &self.config)?;
        host::call_descriptor(&self.bindings, &mut self.store)
    }

    pub fn stream_id(
        &mut self,
        command: &host::CommandEnvelope,
    ) -> Result<Result<String, host::DomainError>, wasmtime::Error> {
        arm_guest_call(&mut self.store, &self.config)?;
        host::call_stream_id(&self.bindings, &mut self.store, command)
    }

    pub fn open_session(
        &mut self,
        snapshot: Option<&[u8]>,
    ) -> Result<super::session::SimSession<'_, T>, wasmtime::Error> {
        arm_guest_call(&mut self.store, &self.config)?;
        let session = host::create_session(&self.bindings, &mut self.store, snapshot)?;
        Ok(super::session::SimSession {
            bindings: &self.bindings,
            store: &mut self.store,
            config: self.config,
            session,
        })
    }
}
