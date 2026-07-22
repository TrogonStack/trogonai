use thiserror::Error;
use trogon_decider_wasm_runtime::{WasmDeciderEngine, WasmEngineConfig, WasmEngineError};
use trogon_decider_wit::host::{self, Decider};
use wasmtime::component::{Component, Linker};
use wasmtime::{Store, StoreLimits, StoreLimitsBuilder};

use crate::import_check::{ImportCheckError, assert_zero_imports};

/// Failure loading, instantiating, or arming a [`SimHost`]'s wasm component.
#[derive(Debug, Error)]
pub enum SimError {
    /// The compiled component declares an import, so it cannot be treated as a production-shaped
    /// zero-import decider.
    #[error(transparent)]
    Import(#[from] ImportCheckError),
    /// The wasmtime engine could not be configured with the requested resource budget.
    #[error("failed to configure wasmtime engine")]
    Engine(#[from] WasmEngineError),
    /// The component bytes failed to compile.
    #[error("failed to compile wasm component")]
    Compile {
        /// The underlying wasmtime compilation error.
        #[source]
        source: wasmtime::Error,
    },
    /// The compiled component failed to instantiate against the linker.
    #[error("failed to instantiate wasm component")]
    Instantiate {
        /// The underlying wasmtime instantiation error.
        #[source]
        source: wasmtime::Error,
    },
    /// The store's fuel or epoch deadline could not be armed before a guest call.
    #[error("failed to arm guest call resource budget")]
    Arm {
        /// The underlying wasmtime error setting fuel or epoch deadline.
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

    /// Creates a new guest store for `host_state`, arms its resource budget, and instantiates the
    /// component's `Decider` bindings into it.
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

/// A single instantiated guest component paired with its store and resource budget, ready to
/// open sessions and issue guest calls against.
pub struct SimInstance<T: 'static> {
    pub(crate) store: Store<SimGuestState<T>>,
    pub(crate) bindings: Decider,
    pub(crate) config: WasmEngineConfig,
}

impl<T: 'static> SimInstance<T> {
    /// Arms the guest call budget and returns the component's declared module descriptor.
    pub fn descriptor(&mut self) -> Result<host::ModuleDescriptor, wasmtime::Error> {
        arm_guest_call(&mut self.store, &self.config)?;
        host::call_descriptor(&self.bindings, &mut self.store)
    }

    /// Arms the guest call budget and resolves the stream id `command` targets.
    pub fn stream_id(
        &mut self,
        command: &host::CommandEnvelope,
    ) -> Result<Result<String, host::DomainError>, wasmtime::Error> {
        arm_guest_call(&mut self.store, &self.config)?;
        host::call_stream_id(&self.bindings, &mut self.store, command)
    }

    /// Arms the guest call budget and opens a new guest session, optionally seeded from a prior
    /// `snapshot`.
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
