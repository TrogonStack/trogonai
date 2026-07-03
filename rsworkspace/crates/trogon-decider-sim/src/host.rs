use thiserror::Error;
use trogon_decider_wit::host::{self, Decider};
use wasmtime::component::{Component, Linker};
use wasmtime::{Config, Engine, Store};

use crate::import_check::{ImportCheckError, assert_zero_imports};

#[derive(Debug, Error)]
pub enum SimError {
    #[error(transparent)]
    Import(#[from] ImportCheckError),
    #[error("failed to configure wasmtime engine")]
    Engine {
        #[source]
        source: wasmtime::Error,
    },
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
}

/// Loaded decider component ready to instantiate in-memory test sessions.
pub struct SimHost {
    engine: Engine,
    component: Component,
}

impl SimHost {
    pub fn load(bytes: &[u8]) -> Result<Self, SimError> {
        assert_zero_imports(bytes)?;

        let mut config = Config::new();
        config.wasm_component_model(true);
        let engine = Engine::new(&config).map_err(|source| SimError::Engine { source })?;
        let component = Component::new(&engine, bytes).map_err(|source| SimError::Compile { source })?;

        Ok(Self { engine, component })
    }

    pub fn instantiate<T: 'static>(&self, host_state: T) -> Result<SimInstance<T>, SimError> {
        let linker = Linker::new(&self.engine);
        let mut store = Store::new(&self.engine, host_state);
        let bindings = Decider::instantiate(&mut store, &self.component, &linker)
            .map_err(|source| SimError::Instantiate { source })?;
        Ok(SimInstance { store, bindings })
    }
}

pub struct SimInstance<T: 'static> {
    pub(crate) store: Store<T>,
    pub(crate) bindings: Decider,
}

impl<T: 'static> SimInstance<T> {
    pub fn descriptor(&mut self) -> Result<host::ModuleDescriptor, wasmtime::Error> {
        host::call_descriptor(&self.bindings, &mut self.store)
    }

    pub fn stream_id(
        &mut self,
        command: &host::CommandEnvelope,
    ) -> Result<Result<String, host::DomainError>, wasmtime::Error> {
        host::call_stream_id(&self.bindings, &mut self.store, command)
    }

    pub fn open_session(
        &mut self,
        snapshot: Option<&[u8]>,
    ) -> Result<super::session::SimSession<'_, T>, wasmtime::Error> {
        let session = host::create_session(&self.bindings, &mut self.store, snapshot)?;
        Ok(super::session::SimSession {
            bindings: &self.bindings,
            store: &mut self.store,
            session,
        })
    }
}
