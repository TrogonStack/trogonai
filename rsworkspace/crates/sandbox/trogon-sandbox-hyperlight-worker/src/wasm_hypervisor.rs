use std::path::Path;
use std::sync::Arc;

use hyperlight_wasm::{OciTag, SandboxBuilder, Snapshot, WasmSandbox, is_hypervisor_present};

use crate::constants::{DEFAULT_GUEST_HEAP_BYTES, DEFAULT_GUEST_INPUT_BUFFER_BYTES, DEFAULT_GUEST_OUTPUT_BUFFER_BYTES};
use crate::{Hypervisor, HypervisorError, WasmGuest, WasmHypervisorError};

/// The tag a snapshot carries inside the layout directory the worker ships. The registry
/// addresses the artifact by digest, so this name only has to agree between save and load.
const LAYOUT_TAG: &str = "snapshot";

/// The Hyperlight backend: every guest is a hyperlight-wasm sandbox on KVM, MSHV or WHP.
///
/// A warm runtime is a sandbox with the Wasm runtime loaded and no module. Unloading a lease's
/// module hands the same sandbox back, so the next lease skips building a new one. Snapshots
/// are written and read as OCI image layout directories, which the worker packs into a single
/// registry layer.
#[derive(Debug, Clone, Copy)]
pub struct WasmHypervisor {
    guest_input_buffer_bytes: usize,
    guest_output_buffer_bytes: usize,
    guest_heap_bytes: u64,
}

impl WasmHypervisor {
    /// A backend whose guests get `guest_input_buffer_bytes` of input buffer,
    /// `guest_output_buffer_bytes` of output buffer and `guest_heap_bytes` of heap.
    pub fn new(guest_input_buffer_bytes: usize, guest_output_buffer_bytes: usize, guest_heap_bytes: u64) -> Self {
        Self {
            guest_input_buffer_bytes,
            guest_output_buffer_bytes,
            guest_heap_bytes,
        }
    }
}

impl Default for WasmHypervisor {
    /// A 256 KiB input buffer, a 256 KiB output buffer and a 1.25 MiB heap per guest.
    fn default() -> Self {
        Self::new(
            DEFAULT_GUEST_INPUT_BUFFER_BYTES,
            DEFAULT_GUEST_OUTPUT_BUFFER_BYTES,
            DEFAULT_GUEST_HEAP_BYTES,
        )
    }
}

fn layout_tag() -> Result<OciTag, WasmHypervisorError> {
    OciTag::new(LAYOUT_TAG).map_err(WasmHypervisorError::LayoutTag)
}

impl Hypervisor for WasmHypervisor {
    type Warm = WasmSandbox;
    type Guest = WasmGuest;

    fn is_available(&self) -> bool {
        is_hypervisor_present()
    }

    fn build_warm(&self) -> Result<Self::Warm, HypervisorError> {
        let proto = SandboxBuilder::new()
            .with_guest_input_buffer_size(self.guest_input_buffer_bytes)
            .with_guest_output_buffer_size(self.guest_output_buffer_bytes)
            .with_guest_heap_size(self.guest_heap_bytes)
            .build()
            .map_err(WasmHypervisorError::Build)?;
        Ok(proto.load_runtime().map_err(WasmHypervisorError::LoadRuntime)?)
    }

    fn load_module(&self, warm: Self::Warm, component: &Path) -> Result<Self::Guest, HypervisorError> {
        let sandbox = warm
            .load_module(component)
            .map_err(|source| WasmHypervisorError::LoadModule {
                path: component.to_path_buf(),
                source,
            })?;
        Ok(WasmGuest::loaded(sandbox)?)
    }

    fn load_layout(&self, warm: Self::Warm, layout: &Path) -> Result<Self::Guest, HypervisorError> {
        let snapshot =
            Snapshot::checked_load(layout, layout_tag()?).map_err(|source| WasmHypervisorError::ReadLayout {
                path: layout.to_path_buf(),
                source,
            })?;
        let snapshot = Arc::new(snapshot);
        let sandbox = warm
            .load_from_snapshot(Arc::clone(&snapshot))
            .map_err(WasmHypervisorError::LoadSnapshot)?;
        Ok(WasmGuest::restored(sandbox, snapshot))
    }

    fn save_layout(&self, guest: &mut Self::Guest, layout: &Path) -> Result<(), HypervisorError> {
        let snapshot = guest.sandbox_mut().snapshot().map_err(WasmHypervisorError::Snapshot)?;
        snapshot
            .save(layout, &layout_tag()?)
            .map_err(|source| WasmHypervisorError::WriteLayout {
                path: layout.to_path_buf(),
                source,
            })?;
        Ok(())
    }

    fn unload(&self, guest: Self::Guest) -> Result<Self::Warm, HypervisorError> {
        Ok(guest
            .into_sandbox()
            .unload_module()
            .map_err(WasmHypervisorError::Unload)?)
    }
}
