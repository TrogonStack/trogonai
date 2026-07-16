use std::collections::HashMap;
use std::sync::{Arc, PoisonError, RwLock, RwLockWriteGuard};

use thiserror::Error;

use crate::{CommandType, ModuleName, ModuleVersion, WasmDeciderModule};

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

/// One command type's current route: the identity of the module handling it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct RouteInfo {
    pub command_type: CommandType,
    pub module_name: ModuleName,
    pub module_version: ModuleVersion,
}

/// One command type's outcome from a [`DeciderRegistryHandle::activate`] call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivatedRoute {
    pub command_type: CommandType,
    /// The module that owned this route immediately before the swap, if any.
    pub previous_module: Option<(ModuleName, ModuleVersion)>,
}

/// The route removed by a [`DeciderRegistryHandle::retire`] call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetiredRoute {
    pub command_type: CommandType,
    pub module_name: ModuleName,
    pub module_version: ModuleVersion,
}

/// Routes command types to the WASM decider module that declared them.
///
/// Built once at startup via [`DeciderRegistryBuilder`] and shared read-only
/// across command executions. See [`DeciderRegistryHandle`] for a form of
/// this registry that can be updated while executions are in flight.
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

    /// Lists every currently routed command type alongside the identity of
    /// the module handling it.
    pub fn routes(&self) -> Vec<RouteInfo> {
        self.modules_by_command
            .iter()
            .map(|(command_type, module)| RouteInfo {
                command_type: command_type.clone(),
                module_name: module.name().clone(),
                module_version: module.version().clone(),
            })
            .collect()
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

    /// Finishes building a [`DeciderRegistryHandle`] wrapping this registry,
    /// ready for runtime rollout via [`DeciderRegistryHandle::activate`] and
    /// [`DeciderRegistryHandle::retire`].
    pub fn build_handle(self) -> DeciderRegistryHandle {
        DeciderRegistryHandle::new(self.build())
    }
}

/// A [`DeciderRegistry`] behind a handle that can be updated while command
/// executions are in flight.
///
/// This crate has no workspace dependency on a lock-free swap primitive such
/// as `arc-swap`, so this handle is built from a
/// `std::sync::RwLock<Arc<DeciderRegistry>>` instead, following the same
/// poison-recovering lock idiom already used elsewhere in this workspace.
///
/// # Atomicity
///
/// The handle holds one [`Arc<DeciderRegistry>`] behind a [`RwLock`]. Every
/// read ([`Self::snapshot`], [`Self::route`], [`Self::routes`]) clones that
/// `Arc` under a brief read lock and then works against its own owned clone,
/// independent of whatever [`Self::activate`] or [`Self::retire`] does next.
/// [`Self::activate`] and [`Self::retire`] each compute the entire next
/// routing table first and take the write lock just long enough to install
/// it with one assignment. A concurrent reader therefore always observes one
/// complete, self-consistent [`DeciderRegistry`]: either every route the last
/// completed write installed, or every route the write before it installed,
/// never a mix of the two. In particular, a module that declares more than
/// one command type is activated or retired as a unit: a reader can never
/// see one of its command types already pointing at a new module version
/// while a sibling command type from that same module still points at the
/// old one. Lock poisoning from a panicked writer is recovered rather than
/// propagated (`RwLock::read`/`write().unwrap_or_else(PoisonError::into_inner)`),
/// so a prior panic elsewhere can never turn a routing lookup into a panic
/// of its own.
///
/// # Dispatch must pin its own resolution
///
/// [`Self::activate`] and [`Self::retire`] only ever affect *future* calls to
/// [`Self::route`] and [`Self::snapshot`]. A caller must resolve the module
/// once per command dispatch and keep using that resolved
/// `Arc<WasmDeciderModule>` (or `Arc<DeciderRegistry>`, from
/// [`Self::snapshot`]) for the rest of that command's execution. Nothing
/// about a later swap reaches back into an execution already running
/// against a module it resolved earlier: the `Arc` it is holding keeps that
/// exact module alive and unchanged no matter how many times the handle is
/// swapped afterward.
///
/// # Snapshot behavior across a version swap
///
/// [`crate::WasmSnapshotId`] folds a module's name and version into every
/// snapshot id it produces (`{module_name}@{module_version}/{stream_id}`).
/// Rerouting a command type from module version `v1` to `v2` via
/// [`Self::activate`] does not migrate or invalidate `v1`'s snapshots: `v2`'s
/// snapshot id for a given stream is simply a different string. The first
/// command `v2` executes against a stream `v1` had already snapshotted reads
/// the snapshot store under `v2`'s id, finds nothing, and falls back to a
/// full replay of that stream's history from the beginning (the same
/// fallback `execution::WasmCommandExecution` already takes for a stream
/// that was never snapshotted at all). That replay reconstructs the exact
/// same guest state a snapshot would have, only cold: every prior event is
/// read and replayed once more instead of resuming from a saved position.
/// `v2` then writes its own snapshot under its own id, so `v1`'s snapshot is
/// left untouched; routing back to `v1` later finds that snapshot exactly
/// where it left it. No migration step, explicit invalidation, or
/// coordination between versions is required: the version bump folded into
/// the snapshot id is enough on its own. See the `activating_a_new_module_version_*`
/// test in this module's `tests` submodule for this behavior exercised end
/// to end.
pub struct DeciderRegistryHandle {
    current: RwLock<Arc<DeciderRegistry>>,
}

impl DeciderRegistryHandle {
    /// Wraps an existing [`DeciderRegistry`] in a swappable handle.
    pub fn new(registry: DeciderRegistry) -> Self {
        Self {
            current: RwLock::new(Arc::new(registry)),
        }
    }

    /// Returns the [`DeciderRegistry`] this handle currently points at.
    ///
    /// Hold the returned `Arc` for the lifetime of one command dispatch so a
    /// concurrent [`Self::activate`] or [`Self::retire`] cannot change which
    /// module that dispatch resolves to partway through.
    pub fn snapshot(&self) -> Arc<DeciderRegistry> {
        self.read()
    }

    /// Looks up the module currently responsible for the given command type.
    ///
    /// Returns an owned `Arc`, independent of any later [`Self::activate`] or
    /// [`Self::retire`] call, so the caller can keep executing against
    /// exactly the module it resolved here.
    pub fn route(&self, command_type: &CommandType) -> Result<Arc<WasmDeciderModule>, UnknownCommandTypeError> {
        self.read().route(command_type).map(Arc::clone)
    }

    /// Lists every currently routed command type alongside the identity of
    /// the module handling it.
    pub fn routes(&self) -> Vec<RouteInfo> {
        self.read().routes()
    }

    /// Routes every command type `module` declares to `module`, replacing
    /// whatever module previously owned each of those routes.
    ///
    /// See the type-level docs for what this means for in-flight executions
    /// and for a stream's snapshots taken under a route's previous module
    /// version.
    pub fn activate(&self, module: WasmDeciderModule) -> Vec<ActivatedRoute> {
        let module = Arc::new(module);
        let command_types = module.command_types().cloned().collect::<Vec<_>>();

        let mut current = self.write();
        let mut modules_by_command = current.modules_by_command.clone();
        let outcomes = command_types
            .into_iter()
            .map(|command_type| {
                let previous = modules_by_command.insert(command_type.clone(), Arc::clone(&module));
                ActivatedRoute {
                    command_type,
                    previous_module: previous.map(|previous| (previous.name().clone(), previous.version().clone())),
                }
            })
            .collect();
        *current = Arc::new(DeciderRegistry { modules_by_command });
        outcomes
    }

    /// Removes the route for one command type, if any module currently owns it.
    ///
    /// A command dispatched for `command_type` after this call fails with
    /// [`UnknownCommandTypeError`] until a later [`Self::activate`] routes it
    /// again.
    pub fn retire(&self, command_type: &CommandType) -> Option<RetiredRoute> {
        let mut current = self.write();
        let mut modules_by_command = current.modules_by_command.clone();
        let removed = modules_by_command.remove(command_type)?;
        *current = Arc::new(DeciderRegistry { modules_by_command });
        Some(RetiredRoute {
            command_type: command_type.clone(),
            module_name: removed.name().clone(),
            module_version: removed.version().clone(),
        })
    }

    fn read(&self) -> Arc<DeciderRegistry> {
        Arc::clone(&self.current.read().unwrap_or_else(PoisonError::into_inner))
    }

    fn write(&self) -> RwLockWriteGuard<'_, Arc<DeciderRegistry>> {
        self.current.write().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Default for DeciderRegistryHandle {
    fn default() -> Self {
        Self::new(DeciderRegistry::default())
    }
}

impl From<DeciderRegistry> for DeciderRegistryHandle {
    fn from(registry: DeciderRegistry) -> Self {
        Self::new(registry)
    }
}

#[cfg(test)]
mod tests;
