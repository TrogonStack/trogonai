use std::collections::HashSet;

use thiserror::Error;
use trogon_decider_wit::host::{DeciderPre, ModuleDescriptor};
use wasmtime::component::{Component, Linker};

use crate::{ModuleName, ModuleVersion, WasmDeciderEngine};

/// Failure loading a compiled WASM decider component.
#[derive(Debug, Error)]
pub enum LoadWasmDeciderError {
    /// The component declares at least one import. Production deciders must be
    /// zero-import; this is the structural analog of the sim crate's
    /// `assert_zero_imports` check, enforced by instantiating against an empty
    /// [`Linker`] instead of shelling out to `wasm-tools`.
    #[error("component declares an import; wasm decider components must be zero-import")]
    ForbiddenImports {
        #[source]
        source: wasmtime::Error,
    },
    #[error("failed to compile wasm component")]
    Compile {
        #[source]
        source: wasmtime::Error,
    },
    #[error("failed to instantiate wasm component to probe its descriptor")]
    Instantiate {
        #[source]
        source: wasmtime::Error,
    },
    #[error("failed to call descriptor() on wasm component")]
    Descriptor {
        #[source]
        source: wasmtime::Error,
    },
    #[error("wasm component descriptor is invalid")]
    InvalidDescriptor(#[from] InvalidDescriptorError),
}

/// A structurally invalid [`ModuleDescriptor`] reported by a guest component.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum InvalidDescriptorError {
    #[error("wasm component descriptor has an invalid module name")]
    InvalidName(#[source] crate::ModuleNameError),
    #[error("wasm component descriptor has an invalid module version")]
    InvalidVersion(#[source] crate::ModuleVersionError),
    #[error("wasm component descriptor declares command type '{command_type}' more than once")]
    DuplicateCommandType { command_type: String },
}

/// A compiled, load-time-validated WASM decider component.
///
/// Loading compiles the component, structurally enforces zero imports by
/// instantiating against an empty [`Linker`], caches the resulting
/// [`DeciderPre`] for cheap per-command instantiation, and probes the
/// guest's `descriptor()` export once so routing and validation do not need
/// to spin up a guest session.
pub struct WasmDeciderModule {
    engine: WasmDeciderEngine,
    decider_pre: DeciderPre<crate::engine::GuestState>,
    descriptor: ModuleDescriptor,
    name: ModuleName,
    version: ModuleVersion,
}

impl WasmDeciderModule {
    /// Compiles and validates a WASM decider component from its bytes.
    pub fn load(engine: WasmDeciderEngine, bytes: &[u8]) -> Result<Self, LoadWasmDeciderError> {
        let component =
            Component::new(engine.engine(), bytes).map_err(|source| LoadWasmDeciderError::Compile { source })?;

        let linker: Linker<crate::engine::GuestState> = Linker::new(engine.engine());
        let instance_pre = linker
            .instantiate_pre(&component)
            .map_err(|source| LoadWasmDeciderError::ForbiddenImports { source })?;
        let decider_pre =
            DeciderPre::new(instance_pre).map_err(|source| LoadWasmDeciderError::Instantiate { source })?;

        let descriptor = probe_descriptor(&engine, &decider_pre)?;
        let name = ModuleName::new(&descriptor.name).map_err(InvalidDescriptorError::InvalidName)?;
        let version = ModuleVersion::new(&descriptor.version).map_err(InvalidDescriptorError::InvalidVersion)?;
        ensure_unique_command_types(&descriptor)?;

        Ok(Self {
            engine,
            decider_pre,
            descriptor,
            name,
            version,
        })
    }

    /// Returns the module's declared name.
    pub fn name(&self) -> &ModuleName {
        &self.name
    }

    /// Returns the module's declared version.
    pub fn version(&self) -> &ModuleVersion {
        &self.version
    }

    /// Returns the cached module descriptor probed at load time.
    pub fn descriptor(&self) -> &ModuleDescriptor {
        &self.descriptor
    }

    /// Returns the command types this module declares handling.
    pub fn command_types(&self) -> impl Iterator<Item = &str> {
        self.descriptor.commands.iter().map(|spec| spec.command_type.as_str())
    }

    pub(crate) fn engine(&self) -> &WasmDeciderEngine {
        &self.engine
    }

    pub(crate) fn decider_pre(&self) -> &DeciderPre<crate::engine::GuestState> {
        &self.decider_pre
    }
}

fn probe_descriptor(
    engine: &WasmDeciderEngine,
    decider_pre: &DeciderPre<crate::engine::GuestState>,
) -> Result<ModuleDescriptor, LoadWasmDeciderError> {
    let mut store = engine.new_store();
    store
        .set_fuel(engine.config().fuel_per_call())
        .map_err(|source| LoadWasmDeciderError::Instantiate { source })?;
    let bindings = decider_pre
        .instantiate(&mut store)
        .map_err(|source| LoadWasmDeciderError::Instantiate { source })?;
    trogon_decider_wit::host::call_descriptor(&bindings, &mut store)
        .map_err(|source| LoadWasmDeciderError::Descriptor { source })
}

fn ensure_unique_command_types(descriptor: &ModuleDescriptor) -> Result<(), InvalidDescriptorError> {
    let mut seen = HashSet::with_capacity(descriptor.commands.len());
    for spec in &descriptor.commands {
        if !seen.insert(spec.command_type.as_str()) {
            return Err(InvalidDescriptorError::DuplicateCommandType {
                command_type: spec.command_type.clone(),
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
