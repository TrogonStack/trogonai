/// Why a WASM guest could not be set up or run to an outcome.
#[derive(Debug, thiserror::Error)]
pub enum WasmRuntimeError {
    #[error("{0}")]
    Setup(String),
    #[error("wasm execution task panicked: {0}")]
    TaskPanicked(String),
}

impl WasmRuntimeError {
    pub(crate) fn setup(error: impl std::fmt::Display) -> Self {
        Self::Setup(error.to_string())
    }
}
