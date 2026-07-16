#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]
//! Production host runtime for compiled WASM Trogon decider components.
//!
//! This crate loads a compiled `trogon:decider@0.1.0` component, structurally
//! enforces that it declares zero imports, and executes commands against it
//! end to end: replaying stream history into a fresh guest session, deciding
//! the command, appending the resulting events, and best-effort snapshotting
//! guest state.
//!
//! # Why not the typed `Decider` trait
//!
//! [`trogon_decider_runtime::execution::CommandExecution`] adapts a native,
//! statically typed [`trogon_decider::Decider`] implementation. A WASM module
//! cannot be adapted behind that trait: its session state lives behind
//! `&mut wasmtime::Store` and a `wasmtime::component::ResourceAny` handle, not
//! a value the trait's static `evolve`/`decide` functions could address; its
//! write preconditions are declared per command in the module descriptor
//! rather than as one `Decider::WRITE_PRECONDITION` const; and its event
//! payloads are already encoded bytes crossing the WIT boundary, so a host
//! `EventEncode`/`EventDecode` adapter would only be an identity codec. This
//! crate reuses the storage-neutral ports from `trogon_decider_runtime`
//! ([`StreamRead`], [`StreamAppend`], [`SnapshotRead`], [`SnapshotWrite`],
//! [`SnapshotTaskScheduler`]) and reimplements execution against the WIT
//! `session` resource directly. See [`execution`] for the full flow.
//!
//! # Zero-import enforcement
//!
//! `trogon-decider-sim` enforces zero imports for tests by shelling out to
//! `wasm-tools`, a dev-time binary that production hosts will not have
//! installed. This crate enforces the same contract structurally instead: at
//! load time it instantiates the component against an empty
//! `wasmtime::component::Linker`. A component that declares any import fails
//! resolution against that empty linker, so only a zero-import component can
//! load. See [`module::LoadWasmDeciderError::ForbiddenImports`].
//!
//! # Snapshot identity
//!
//! [`snapshot::SnapshotType::snapshot_type`] is a static function, so it
//! cannot see a runtime-loaded module's identity. This crate uses one fixed
//! [`OpaqueSnapshotPayload`] snapshot type for every WASM module and instead
//! folds module identity into the snapshot id: see [`WasmSnapshotId`]. A
//! module version bump changes the id every snapshot lookup uses, so an old
//! snapshot is simply not found and execution falls back to a full replay.
//!
//! # Runtime rollout
//!
//! [`DeciderRegistry`] is immutable once built: swapping in a new module
//! version means building and installing a whole new registry. Callers that
//! need to change routes while commands are in flight, without dropping
//! executions already dispatched against the old routes, should build a
//! [`DeciderRegistryHandle`] instead (via [`DeciderRegistryBuilder::build_handle`]
//! or [`DeciderRegistryHandle::from`]). See its type-level docs for the
//! atomicity guarantee the handle provides and for what a version swap means
//! for a stream's snapshots.

mod command_spec;
mod command_type;
mod domain_error_detail;
mod engine;
mod execution;
mod module;
mod module_name;
mod module_version;
mod opaque_snapshot;
mod registry;
mod snapshot_id;

#[cfg(test)]
mod test_doubles;
#[cfg(test)]
mod test_fixture;

pub use command_spec::WasmCommandSpec;
pub use command_type::{CommandType, CommandTypeError};
pub use domain_error_detail::DomainErrorDetail;
pub use engine::{
    DEFAULT_EPOCH_TICK_INTERVAL, DEFAULT_EPOCH_TICKS_PER_CALL, DEFAULT_FUEL_PER_CALL, DEFAULT_MAX_CONCURRENT_SESSIONS,
    DEFAULT_MAX_INSTANCES_PER_SESSION, DEFAULT_MAX_MEMORIES_PER_SESSION, DEFAULT_MAX_MEMORY_BYTES,
    DEFAULT_MAX_TABLE_ELEMENTS, DEFAULT_MAX_TABLES_PER_SESSION, WasmDeciderEngine, WasmEngineConfig, WasmEngineError,
};
pub use execution::{
    WasmCommandError, WasmCommandExecution, WasmExecutionResult, WithSnapshotStore, WithoutSnapshotStore,
};
pub use module::{InvalidDescriptorError, LoadWasmDeciderError, WasmDeciderModule};
pub use module_name::{ModuleName, ModuleNameError};
pub use module_version::{ModuleVersion, ModuleVersionError};
pub use opaque_snapshot::OpaqueSnapshotPayload;
pub use registry::{
    ActivatedRoute, DeciderRegistry, DeciderRegistryBuilder, DeciderRegistryHandle, RegisterModuleError, RetiredRoute,
    RouteInfo, UnknownCommandTypeError,
};
pub use snapshot_id::WasmSnapshotId;
