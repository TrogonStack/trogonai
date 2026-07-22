use std::collections::HashSet;

use thiserror::Error;
use trogon_decider_wit::host::{DeciderPre, ModuleDescriptor};
use wasmtime::component::{Component, Linker};

use crate::{CommandType, ModuleName, ModuleVersion, WasmCommandSpec, WasmDeciderEngine};

/// Failure loading a compiled WASM decider component.
#[derive(Debug, Error)]
pub enum LoadWasmDeciderError {
    /// The component declares at least one import. Production deciders must be
    /// zero-import; this is the structural analog of the sim crate's
    /// `assert_zero_imports` check, enforced by instantiating against an empty
    /// [`Linker`] instead of shelling out to `wasm-tools`.
    #[error("component declares an import; wasm decider components must be zero-import")]
    ForbiddenImports {
        /// Underlying wasmtime error raised instantiating against an empty [`Linker`].
        #[source]
        source: wasmtime::Error,
    },
    /// The component's bytes are not a valid wasm component, or targeted a wasmtime feature
    /// this engine was not configured with.
    #[error("failed to compile wasm component")]
    Compile {
        /// Underlying wasmtime compilation error.
        #[source]
        source: wasmtime::Error,
    },
    /// Instantiating the pre-instantiated component against a fresh store failed.
    #[error("failed to instantiate wasm component to probe its descriptor")]
    Instantiate {
        /// Underlying wasmtime instantiation error.
        #[source]
        source: wasmtime::Error,
    },
    /// The guest's `descriptor()` export trapped or exceeded its call budget.
    #[error("failed to call descriptor() on wasm component")]
    Descriptor {
        /// Underlying wasmtime error from the guest call.
        #[source]
        source: wasmtime::Error,
    },
    /// The guest's declared [`ModuleDescriptor`] failed structural validation.
    #[error("wasm component descriptor is invalid")]
    InvalidDescriptor(#[from] InvalidDescriptorError),
}

/// A structurally invalid [`ModuleDescriptor`] reported by a guest component.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum InvalidDescriptorError {
    /// The descriptor's `name` field is not a well-formed [`ModuleName`].
    #[error("wasm component descriptor has an invalid module name")]
    InvalidName(#[source] crate::ModuleNameError),
    /// The descriptor's `version` field is not a well-formed [`ModuleVersion`].
    #[error("wasm component descriptor has an invalid module version")]
    InvalidVersion(#[source] crate::ModuleVersionError),
    /// One of the descriptor's declared commands has an invalid command type string.
    #[error("wasm component descriptor declares an invalid command type")]
    InvalidCommandType(#[source] crate::CommandTypeError),
    /// The descriptor declares the same command type more than once.
    #[error("wasm component descriptor declares command type '{command_type}' more than once")]
    DuplicateCommandType {
        /// The command type declared more than once.
        command_type: CommandType,
    },
}

/// A compiled, load-time-validated WASM decider component.
///
/// Loading compiles the component, structurally enforces zero imports by
/// instantiating against an empty [`Linker`], caches the resulting
/// [`DeciderPre`] for cheap per-command instantiation, and probes the
/// guest's `descriptor()` export once so routing and validation do not need
/// to spin up a guest session.
#[derive(Clone)]
pub struct WasmDeciderModule {
    engine: WasmDeciderEngine,
    decider_pre: DeciderPre<crate::engine::GuestState>,
    commands: Vec<WasmCommandSpec>,
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
        let commands = validate_commands(descriptor)?;

        Ok(Self {
            engine,
            decider_pre,
            commands,
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

    /// Returns the load-time-validated command declarations.
    pub fn commands(&self) -> &[WasmCommandSpec] {
        &self.commands
    }

    /// Returns the command types this module declares handling.
    pub fn command_types(&self) -> impl Iterator<Item = &CommandType> {
        self.commands.iter().map(WasmCommandSpec::command_type)
    }

    pub(crate) fn engine(&self) -> &WasmDeciderEngine {
        &self.engine
    }

    pub(crate) fn decider_pre(&self) -> &DeciderPre<crate::engine::GuestState> {
        &self.decider_pre
    }
}

#[cfg(test)]
impl WasmDeciderModule {
    /// Returns a copy of this module reporting a different declared version.
    ///
    /// The only compiled fixture available to this crate's tests always
    /// reports one fixed identity, so registry and rollout tests that need a
    /// second, distinct module version synthesize one from the same compiled
    /// bytes rather than building a second fixture.
    pub(crate) fn with_version(mut self, version: ModuleVersion) -> Self {
        self.version = version;
        self
    }
}

fn probe_descriptor(
    engine: &WasmDeciderEngine,
    decider_pre: &DeciderPre<crate::engine::GuestState>,
) -> Result<ModuleDescriptor, LoadWasmDeciderError> {
    let mut store = engine.new_store();
    engine
        .arm_guest_call(
            &mut store,
            engine.config().fuel_per_call(),
            engine.config().epoch_ticks_per_call(),
        )
        .map_err(|source| LoadWasmDeciderError::Instantiate { source })?;
    let bindings = decider_pre
        .instantiate(&mut store)
        .map_err(|source| LoadWasmDeciderError::Instantiate { source })?;
    trogon_decider_wit::host::call_descriptor(&bindings, &mut store)
        .map_err(|source| LoadWasmDeciderError::Descriptor { source })
}

fn validate_commands(descriptor: ModuleDescriptor) -> Result<Vec<WasmCommandSpec>, InvalidDescriptorError> {
    let mut seen = HashSet::with_capacity(descriptor.commands.len());
    let mut commands = Vec::with_capacity(descriptor.commands.len());
    for spec in descriptor.commands {
        let command_type = CommandType::new(spec.command_type).map_err(InvalidDescriptorError::InvalidCommandType)?;
        if !seen.insert(command_type.clone()) {
            return Err(InvalidDescriptorError::DuplicateCommandType { command_type });
        }
        commands.push(WasmCommandSpec::new(command_type, spec.write_precondition));
    }
    Ok(commands)
}

#[cfg(test)]
mod tests;
