use std::path::Path;

use crate::HypervisorError;

/// The part of a hypervisor-backed Wasm runtime the worker needs.
///
/// The worker owns every decision around it: which lease holds which guest, how many warm
/// runtimes to keep, how snapshots travel and what the authority guard may purge. An
/// implementation only drives the hypervisor. Every method may block, so the worker calls
/// them from the blocking thread pool.
pub trait Hypervisor: Send + Sync + 'static {
    /// A runtime that is built and has no Wasm module loaded.
    type Warm: Send + 'static;
    /// A runtime with a module loaded, held by one lease.
    type Guest: Send + 'static;

    /// Whether this host can run guests at all.
    fn is_available(&self) -> bool;

    /// Builds a runtime with no module loaded.
    fn build_warm(&self) -> Result<Self::Warm, HypervisorError>;

    /// Loads the Wasm component at `component`, consuming the runtime.
    fn load_module(&self, warm: Self::Warm, component: &Path) -> Result<Self::Guest, HypervisorError>;

    /// Restores a guest from a snapshot layout directory, consuming the runtime.
    fn load_layout(&self, warm: Self::Warm, layout: &Path) -> Result<Self::Guest, HypervisorError>;

    /// Writes a snapshot of `guest` as a layout directory at `layout`.
    fn save_layout(&self, guest: &mut Self::Guest, layout: &Path) -> Result<(), HypervisorError>;

    /// Unloads the module and returns the runtime for reuse.
    fn unload(&self, guest: Self::Guest) -> Result<Self::Warm, HypervisorError>;
}
