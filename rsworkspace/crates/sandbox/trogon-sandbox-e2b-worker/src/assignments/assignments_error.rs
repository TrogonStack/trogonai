use async_nats::jetstream::kv;

use crate::custody_bucket_error::CustodyBucketError;
use crate::native_sandbox_id_error::NativeSandboxIdError;

/// Why an assignment could not be stored, read or trusted.
#[derive(Debug, thiserror::Error)]
pub enum AssignmentsError {
    #[error("assignment storage was already configured")]
    AlreadyConfigured,
    #[error("E2B assignment storage is not configured")]
    NotConfigured,
    #[error("assignment storage unavailable")]
    Bucket(#[from] CustodyBucketError),
    #[error("assignment lacks a canonical native sandbox identity")]
    SandboxId(#[from] NativeSandboxIdError),
    #[error("unsupported E2B assignment version")]
    UnsupportedVersion,
    #[error("E2B assignment requires an exact worker and nonzero epoch")]
    NotExact,
    #[error("native assignment lacks recovery metadata")]
    MissingRecovery,
    #[error("invalid assignment lifetime metadata")]
    InvalidLifetime,
    #[error("invalid assignment region metadata")]
    InvalidRegion,
    #[error("E2B assignment exceeds its storage budget")]
    TooLarge,
    #[error("assignment has an unreadable authority marker")]
    UnreadableMarker,
    #[error("assignment identity changed")]
    IdentityChanged,
    #[error("assignment tenant changed")]
    TenantChanged,
    #[error("assignment recovery metadata changed")]
    RecoveryChanged,
    #[error("assignment native identity changed")]
    NativeIdentityChanged,
    #[error("native receipt lacks assignment ownership")]
    LacksOwnership,
    #[error("assignment ownership is changing concurrently")]
    Contended,
    #[error("assignment native custody is changing concurrently")]
    NativeContended,
    #[error("assignment record could not be encoded or decoded")]
    Codec(#[from] serde_json::Error),
    #[error("assignment record could not be read")]
    Read(#[from] kv::EntryError),
    #[error("assignment record could not be created")]
    Create(#[from] kv::CreateError),
    #[error("assignment record could not be updated")]
    Update(#[from] kv::UpdateError),
}
