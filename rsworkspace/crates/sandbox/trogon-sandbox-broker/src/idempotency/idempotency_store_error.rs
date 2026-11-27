use async_nats::jetstream;
use async_nats::jetstream::kv;

/// Why an idempotency or dispatch journal store call failed.
#[derive(Debug, thiserror::Error)]
pub enum IdempotencyStoreError {
    #[error("idempotency bucket unavailable: {0}")]
    Bucket(#[from] jetstream::context::CreateKeyValueError),
    #[error("idempotency entry read failed: {0}")]
    Read(#[from] kv::EntryError),
    #[error("idempotency entry create failed: {0}")]
    Create(#[from] kv::CreateError),
    #[error("dispatch journal update or retirement failed: {0}")]
    Update(#[from] kv::UpdateError),
    #[error("idempotency entry is not valid JSON: {0}")]
    Encoding(#[from] serde_json::Error),
    #[error("dispatch journal identity does not match its request key")]
    JournalIdentityMismatch,
    #[error("new dispatch must be pending")]
    NewDispatchNotPending,
    #[error("expected dispatch attempt missing")]
    ExpectedAttemptMissing,
    #[error("replacement dispatch attempt missing")]
    ReplacementAttemptMissing,
    #[error("dispatch replacement cannot change attempt identity")]
    ReplacementChangesIdentity,
    #[error("dispatch replacement cannot change its first cleanup reason")]
    ReplacementChangesCleanupReason,
    #[error("dispatch replacement cannot change its first cleanup time")]
    ReplacementChangesCleanupTime,
    #[error("dispatch journaling is unavailable")]
    JournalUnavailable,
    #[error("dispatch journal compare-and-swap is unavailable")]
    JournalCasUnavailable,
    #[error("dispatch journal retirement is unavailable")]
    JournalRetirementUnavailable,
    #[error("dispatch journal update exhausted CAS retries")]
    UpdateRetriesExhausted,
    #[error("dispatch journal retirement exhausted CAS retries")]
    RetirementRetriesExhausted,
}
