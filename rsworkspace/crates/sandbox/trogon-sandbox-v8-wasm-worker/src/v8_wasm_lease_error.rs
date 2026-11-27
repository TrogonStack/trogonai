use crate::{JsRuntimeError, WasmRuntimeError};

/// Why a V8 or Wasm lease could not be granted.
#[derive(Debug, thiserror::Error)]
pub enum V8WasmLeaseError {
    /// The lease id is not a valid sandbox id.
    #[error(transparent)]
    SandboxId(#[from] trogon_sandbox_contract::SandboxIdError),
    /// The lease id cannot be tracked as an authority resource.
    #[error(transparent)]
    Resource(#[from] trogon_sandbox_protocol::AuthorityResourceIdError),
    /// The lease still holds a guest resource from an earlier allocation.
    #[error("this lease still retains a guest resource; settle cleanup before reallocating")]
    StillRetained,
    /// The worker holds as many persistent runtimes as it may.
    #[error("this worker already holds {0} persistent runtimes")]
    PersistentRuntimeCap(usize),
    /// The workspace is not a host mount.
    #[error("workspace source {0} is not supported by this provider: only host_mount is supported")]
    UnsupportedWorkspace(String),
    /// Neither the image nor the environment names a guest kind.
    #[error("spec.env[V8_WASM_GUEST] is required")]
    MissingGuestKind,
    /// The guest kind is neither `js` nor `wasm`.
    #[error("unsupported V8_WASM_GUEST value {0:?}: expected \"js\" or \"wasm\"")]
    UnsupportedGuestKind(String),
    /// A persistent runtime was requested for a Wasm guest.
    #[error("V8_WASM_PERSISTENT_RUNTIME is only supported for js guests")]
    PersistentRuntimeNeedsJs,
    /// The guest source could not be read.
    #[error("failed to read {path}: {source}")]
    ReadSource {
        /// The file that could not be read.
        path: String,
        /// Why the read failed.
        source: std::io::Error,
    },
    /// The JS source is not UTF-8.
    #[error("index.js is not valid UTF-8: {0}")]
    SourceNotUtf8(#[from] std::string::FromUtf8Error),
    /// The persistent runtime could not be started.
    #[error("persistent runtime startup task failed: {0}")]
    SessionStartup(#[from] tokio::task::JoinError),
    /// The JS runtime failed.
    #[error(transparent)]
    Js(#[from] JsRuntimeError),
    /// The Wasm runtime failed.
    #[error(transparent)]
    Wasm(#[from] WasmRuntimeError),
    /// The guest ran and failed, timed out or was cancelled mid-run.
    #[error("{0}")]
    Guest(String),
    /// The lease was cancelled before its guest finished.
    #[error("lease execution was cancelled")]
    Cancelled,
    /// The task that runs the guest ended without an answer.
    #[error("guest task ended without an allocation result")]
    TaskEnded,
}
