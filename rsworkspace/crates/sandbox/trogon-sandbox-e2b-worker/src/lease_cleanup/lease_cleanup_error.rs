use async_nats::jetstream::kv;

use crate::custody_bucket_error::CustodyBucketError;
use crate::native_sandbox_id_error::NativeSandboxIdError;
use crate::snapshot_custody::SnapshotClockError;

/// Why an owned-cleanup receipt could not be stored, read or trusted.
#[derive(Debug, thiserror::Error)]
pub enum LeaseCleanupError {
    #[error("owned cleanup receipt context is already configured")]
    AlreadyConfigured,
    #[error("owned cleanup requires worker JetStream custody")]
    NotConfigured,
    #[error("owned cleanup journal unavailable")]
    Bucket(#[from] CustodyBucketError),
    #[error("owned cleanup needs a bounded tenant and lease identity")]
    UnboundedOwner,
    #[error(transparent)]
    SandboxId(#[from] NativeSandboxIdError),
    #[error("owned cleanup receipt exceeds the byte limit")]
    TooLarge,
    #[error("invalid owned cleanup receipt")]
    Decode(#[source] serde_json::Error),
    #[error("owned cleanup receipt could not be encoded")]
    Encode(#[from] serde_json::Error),
    #[error("owned cleanup receipt owner or identity mismatch")]
    OwnerMismatch,
    #[error("absent cleanup receipt has an invalid claim")]
    InvalidClaim,
    #[error("owned cleanup is already in progress")]
    InProgress,
    #[error("owned cleanup journal remained contended after bounded retries")]
    Contended,
    #[error("owned cleanup receipt clock is unavailable")]
    Clock(#[from] SnapshotClockError),
    #[error("owned cleanup receipt could not be read")]
    Read(#[from] kv::EntryError),
    #[error("owned cleanup receipt could not be created")]
    Create(#[from] kv::CreateError),
    #[error("owned cleanup receipt could not be updated")]
    Update(#[from] kv::UpdateError),
}
