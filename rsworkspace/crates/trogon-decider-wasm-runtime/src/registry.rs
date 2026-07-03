use std::collections::HashMap;
use std::sync::Arc;

use thiserror::Error;

use crate::{CommandType, ModuleName, WasmDeciderModule};

/// Failure registering a [`WasmDeciderModule`] into a [`DeciderRegistry`].
#[derive(Debug, Error, PartialEq, Eq)]
#[error(
    "command type '{command_type}' is already routed to module '{existing_module}'; module '{new_module}' cannot also claim it"
)]
pub struct RegisterModuleError {
    pub command_type: CommandType,
    pub existing_module: ModuleName,
    pub new_module: ModuleName,
}

/// Failure routing a command to a registered [`WasmDeciderModule`].
#[derive(Debug, Error, PartialEq, Eq)]
#[error("no wasm decider module is registered for command type '{command_type}'")]
pub struct UnknownCommandTypeError {
    pub command_type: CommandType,
}

/// Routes command types to the WASM decider module that declared them.
///
/// Built once at startup via [`DeciderRegistryBuilder`] and shared read-only
/// across command executions.
#[derive(Default)]
pub struct DeciderRegistry {
    modules_by_command: HashMap<CommandType, Arc<WasmDeciderModule>>,
}

impl DeciderRegistry {
    /// Starts building a registry.
    pub fn builder() -> DeciderRegistryBuilder {
        DeciderRegistryBuilder::default()
    }

    /// Looks up the module responsible for the given command type.
    pub fn route(&self, command_type: &CommandType) -> Result<&Arc<WasmDeciderModule>, UnknownCommandTypeError> {
        self.modules_by_command
            .get(command_type)
            .ok_or_else(|| UnknownCommandTypeError {
                command_type: command_type.clone(),
            })
    }
}

/// Builder for [`DeciderRegistry`] that rejects command type collisions across modules.
#[derive(Default)]
pub struct DeciderRegistryBuilder {
    modules_by_command: HashMap<CommandType, Arc<WasmDeciderModule>>,
}

impl DeciderRegistryBuilder {
    /// Registers every command type declared by the module's descriptor.
    pub fn register(mut self, module: WasmDeciderModule) -> Result<Self, RegisterModuleError> {
        let module = Arc::new(module);
        for command_type in module.command_types().cloned().collect::<Vec<_>>() {
            if let Some(existing) = self.modules_by_command.get(&command_type) {
                return Err(RegisterModuleError {
                    command_type,
                    existing_module: existing.name().clone(),
                    new_module: module.name().clone(),
                });
            }
            self.modules_by_command.insert(command_type, Arc::clone(&module));
        }
        Ok(self)
    }

    /// Finishes building the registry.
    pub fn build(self) -> DeciderRegistry {
        DeciderRegistry {
            modules_by_command: self.modules_by_command,
        }
    }
}

#[cfg(test)]
mod tests;
