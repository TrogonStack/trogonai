//! Sandbox leases as Wasm components in hypervisor-isolated guests.
//!
//! The worker queue-subscribes to Hyperlight lease dispatches, loads the component or snapshot a
//! lease names into a guest and keeps that guest until the lease is cancelled. Everything that
//! touches a hypervisor sits behind the [`Hypervisor`] trait, so the lease rules, the warm pool,
//! the snapshot transport and the authority guard run and are tested without one.
//!
//! - Provider: [`HyperlightWorkerProvider`] implements `Provider` and `AuthorityHost` over a
//!   [`HyperlightRuntime`].
//! - Worker: [`HyperlightWorker`] serves one provider on NATS, and [`launch`] builds it from
//!   [`WorkerArgs`].
//! - Backends: [`UnsupportedHypervisor`] is the backend of a host with no hypervisor; the binary
//!   uses it and exits at startup.

mod cancel_subject;
mod cleanup_handle;
mod component_path;
mod component_path_error;
mod constants;
mod handle_store;
mod handle_store_error;
mod hyperlight_error;
mod hyperlight_runtime;
mod hyperlight_worker;
mod hyperlight_worker_error;
mod hyperlight_worker_provider;
mod hypervisor;
mod hypervisor_error;
mod launch;
mod launch_error;
mod layout_archive;
mod lease_error_reply;
mod lease_gates;
mod profile;
mod retained_handles;
mod runtime_error;
mod runtime_state;
mod sandbox_source;
mod snapshot_artifact;
mod snapshot_transport_error;
mod unsupported_hypervisor;
mod warm_pool;
mod worker_args;

pub use component_path::resolve_component_path;
pub use component_path_error::ComponentPathError;
pub use handle_store_error::HandleStoreError;
pub use hyperlight_error::HyperlightError;
pub use hyperlight_runtime::HyperlightRuntime;
pub use hyperlight_worker::HyperlightWorker;
pub use hyperlight_worker_error::HyperlightWorkerError;
pub use hyperlight_worker_provider::HyperlightWorkerProvider;
pub use hypervisor::Hypervisor;
pub use hypervisor_error::HypervisorError;
pub use launch::{launch, launch_until, launch_with, report, serve};
pub use launch_error::LaunchError;
pub use layout_archive::{pack_layout_dir, unpack_layout_dir};
pub use profile::capability_profile;
pub use runtime_error::RuntimeError;
pub use snapshot_artifact::{
    HYPERLIGHT_SNAPSHOT_MEDIA_TYPE, pull_snapshot_blob, push_snapshot_artifact, snapshot_layer,
};
pub use snapshot_transport_error::SnapshotTransportError;
pub use unsupported_hypervisor::UnsupportedHypervisor;
pub use worker_args::WorkerArgs;

pub(crate) use cleanup_handle::CleanupHandle;
pub(crate) use lease_gates::LeaseGates;
pub(crate) use retained_handles::RetainedHandles;
pub(crate) use warm_pool::WarmPool;
