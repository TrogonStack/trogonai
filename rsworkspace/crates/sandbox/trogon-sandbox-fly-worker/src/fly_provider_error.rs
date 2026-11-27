use trogon_sandbox_contract::{SandboxIdError, TokenError};
use trogon_sandbox_protocol::{AttemptLedgerError, GuestCaptureError};

use crate::{FlyApiError, FlyConfigError, FlySnapshotRefError, FlyVolumeIdError, MachineIdError, MachineSizeError};

/// A lease or lease operation could not be carried out on Fly.
#[derive(Debug, thiserror::Error)]
pub enum FlyProviderError {
    #[error(transparent)]
    Api(#[from] FlyApiError),
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
    #[error(transparent)]
    Config(#[from] FlyConfigError),
    #[error(transparent)]
    Snapshot(#[from] FlySnapshotRefError),
    #[error(transparent)]
    Size(#[from] MachineSizeError),
    #[error("Fly returned an unusable Machine id: {0}")]
    MachineId(#[from] MachineIdError),
    #[error("Fly returned an unusable volume id: {0}")]
    VolumeId(#[from] FlyVolumeIdError),
    #[error("the Machine id is not a sandbox id: {0}")]
    SandboxId(#[from] SandboxIdError),
    #[error(transparent)]
    Capture(#[from] GuestCaptureError),
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
    #[error("cloud create outcome is ambiguous; backend recovery proof is required")]
    Ambiguous,
    #[error("lease is held by a different backend attempt")]
    DifferentAttempt,
    #[error("snapshot {0} failed")]
    SnapshotFailed(String),
}
