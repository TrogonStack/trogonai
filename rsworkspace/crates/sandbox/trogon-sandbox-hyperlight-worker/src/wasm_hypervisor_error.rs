use std::path::PathBuf;

use hyperlight_wasm::HyperlightError;

use crate::HypervisorError;

/// Why a step of the [`WasmHypervisor`](crate::WasmHypervisor) backend failed.
#[derive(Debug, thiserror::Error)]
pub enum WasmHypervisorError {
    /// The sandbox could not be built.
    #[error("building the sandbox failed: {0}")]
    Build(#[source] HyperlightError),
    /// The Wasm runtime could not be loaded into a new sandbox.
    #[error("loading the wasm runtime failed: {0}")]
    LoadRuntime(#[source] HyperlightError),
    /// The guest refused the module at `path`.
    #[error("loading the module {} failed: {source}", path.display())]
    LoadModule {
        path: PathBuf,
        #[source]
        source: HyperlightError,
    },
    /// The guest could not be snapshotted.
    #[error("snapshotting the guest failed: {0}")]
    Snapshot(#[source] HyperlightError),
    /// The layout tag was refused.
    #[error("the snapshot layout tag was refused: {0}")]
    LayoutTag(#[source] HyperlightError),
    /// The layout at `path` did not hold a loadable snapshot.
    #[error("reading the snapshot layout {} failed: {source}", path.display())]
    ReadLayout {
        path: PathBuf,
        #[source]
        source: HyperlightError,
    },
    /// A snapshot could not be written as a layout at `path`.
    #[error("writing the snapshot layout {} failed: {source}", path.display())]
    WriteLayout {
        path: PathBuf,
        #[source]
        source: HyperlightError,
    },
    /// A runtime could not start a guest from a snapshot.
    #[error("starting the guest from a snapshot failed: {0}")]
    LoadSnapshot(#[source] HyperlightError),
    /// The guest could not be rewound to the snapshot it started from.
    #[error("resetting the guest failed: {0}")]
    Reset(#[source] HyperlightError),
    /// The module could not be unloaded to give the runtime back.
    #[error("unloading the module failed: {0}")]
    Unload(#[source] HyperlightError),
}

impl From<WasmHypervisorError> for HypervisorError {
    fn from(error: WasmHypervisorError) -> Self {
        HypervisorError::failed(error)
    }
}

#[cfg(test)]
mod tests;
