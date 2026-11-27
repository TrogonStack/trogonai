use std::sync::Arc;

use hyperlight_wasm::{LoadedWasmSandbox, Snapshot};

use crate::WasmHypervisorError;

/// A Hyperlight sandbox with a module loaded, and the snapshot it started from.
///
/// The snapshot is taken right after the module loads, or is the one the guest was restored
/// from, so [`reset`](Self::reset) always rewinds to the state the lease was granted in.
pub struct WasmGuest {
    sandbox: LoadedWasmSandbox,
    clean: Arc<Snapshot>,
}

impl WasmGuest {
    /// A guest over a freshly loaded module, snapshotting it as its clean state.
    pub(crate) fn loaded(mut sandbox: LoadedWasmSandbox) -> Result<Self, WasmHypervisorError> {
        let clean = sandbox.snapshot().map_err(WasmHypervisorError::Snapshot)?;
        Ok(Self { sandbox, clean })
    }

    /// A guest started from `clean`.
    pub(crate) fn restored(sandbox: LoadedWasmSandbox, clean: Arc<Snapshot>) -> Self {
        Self { sandbox, clean }
    }

    /// Rewinds the guest to the state it was granted in, so each step starts from the same
    /// baseline.
    pub fn reset(&mut self) -> Result<(), WasmHypervisorError> {
        self.sandbox
            .restore(Arc::clone(&self.clean))
            .map_err(WasmHypervisorError::Reset)
    }

    /// The sandbox, to call guest functions on.
    pub fn sandbox_mut(&mut self) -> &mut LoadedWasmSandbox {
        &mut self.sandbox
    }

    /// The sandbox, giving up the clean snapshot.
    pub(crate) fn into_sandbox(self) -> LoadedWasmSandbox {
        self.sandbox
    }
}
