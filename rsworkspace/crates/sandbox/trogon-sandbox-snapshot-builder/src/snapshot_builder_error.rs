use async_nats::{ConnectError, SubscribeError};
use tokio::task::JoinError;
use trogon_sandbox_broker::snapshot_accounting::SnapshotAccountingError;
use trogon_sandbox_broker::snapshot_capture_status::SnapshotCaptureStatusError;
use trogon_sandbox_broker::snapshot_tag::{SnapshotTagAuditError, SnapshotTagStoreError};

use crate::{SnapshotKvError, TagRecoveryError};

/// Why the builder could not start or had to stop.
#[derive(Debug, thiserror::Error)]
pub enum SnapshotBuilderError {
    #[error("failed to connect to NATS at {url}: {source}")]
    Connect {
        url: String,
        #[source]
        source: ConnectError,
    },
    #[error(transparent)]
    Kv(#[from] SnapshotKvError),
    #[error("snapshot tag audit unavailable: {0}")]
    TagAudit(#[from] SnapshotTagAuditError),
    #[error("snapshot tag store unavailable: {0}")]
    TagStore(#[from] SnapshotTagStoreError),
    #[error(transparent)]
    Accounting(#[from] SnapshotAccountingError),
    #[error("snapshot capture status store unavailable: {0}")]
    CaptureStatus(#[from] SnapshotCaptureStatusError),
    #[error("failed to subscribe to lease releases: {0}")]
    Subscribe(#[from] SubscribeError),
    #[error(transparent)]
    Recovery(#[from] TagRecoveryError),
    #[error("snapshot tag recovery task failed: {0}")]
    RecoveryTask(#[from] JoinError),
}
