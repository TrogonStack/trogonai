use trogon_sandbox_contract::{SandboxIdError, TokenError};
use trogon_sandbox_protocol::{AttemptLedgerError, GuestCaptureError};

use crate::{LeaseProxyError, NetworkSetupError, ProxmoxApiError, ProxmoxConfigError, VmIdError};

/// A lease or lease operation could not be carried out on Proxmox.
#[derive(Debug, thiserror::Error)]
pub enum ProxmoxProviderError {
    #[error(transparent)]
    Api(#[from] ProxmoxApiError),
    #[error(transparent)]
    Ledger(#[from] AttemptLedgerError),
    #[error("the worker state directory is unusable: {0}")]
    State(#[from] std::io::Error),
    #[error("another worker owns this state directory")]
    StateLocked,
    #[error("invalid worker ownership record: {0}")]
    OwnerRecord(#[from] uuid::Error),
    #[error("invalid worker id: {0}")]
    WorkerId(#[from] TokenError),
    #[error("the VM id is not a sandbox id: {0}")]
    SandboxId(#[from] SandboxIdError),
    #[error(transparent)]
    Capture(#[from] GuestCaptureError),
    #[error(transparent)]
    Proxy(#[from] LeaseProxyError),
    #[error(transparent)]
    Network(#[from] NetworkSetupError),
    #[error(transparent)]
    Config(#[from] ProxmoxConfigError),
    #[error(transparent)]
    VmId(#[from] VmIdError),
    #[error("a journal could not be read or written: {0}")]
    Journal(#[from] serde_json::Error),
    #[error("guest file content is not valid base64: {0}")]
    Encoding(#[from] base64::DecodeError),
    #[error("the request is unavailable on this worker: {0}")]
    Rejected(&'static str),
    #[error("lease already active or worker full")]
    Capacity,
    #[error("lease is not held by this worker")]
    NotHeld,
    #[error("{0}")]
    Backend(&'static str),
    #[error("{0} timed out")]
    TimedOut(&'static str),
    #[error("{0} does not belong to this worker")]
    Foreign(&'static str),
    #[error("lease is held by a different backend attempt")]
    DifferentAttempt,
    #[error("{launch}; cleanup remains pending: {cleanup}")]
    CleanupPending {
        launch: Box<ProxmoxProviderError>,
        cleanup: Box<ProxmoxProviderError>,
    },
}
