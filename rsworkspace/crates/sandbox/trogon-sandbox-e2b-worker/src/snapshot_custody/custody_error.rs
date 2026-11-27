use async_nats::jetstream::kv;
use async_nats::jetstream::stream::{InfoError, RawMessageError};

use crate::custody_bucket_error::CustodyBucketError;
use crate::snapshots::SnapshotReferenceError;

#[derive(Debug, thiserror::Error)]
pub enum SnapshotCustodyError {
    #[error("snapshot custody context is already configured")]
    AlreadyConfigured,
    #[error("snapshot metadata custody requires worker JetStream context")]
    NotConfigured,
    #[error("snapshot custody bucket is unavailable")]
    Bucket(#[from] CustodyBucketError),
    #[error("invalid snapshot custody owner or version")]
    InvalidVersion,
    #[error(transparent)]
    Reference(#[from] SnapshotReferenceError),
    #[error("invalid custody reference")]
    InvalidReferenceId,
    #[error("snapshot custody lacks exact canonical identity")]
    IdentityMissing,
    #[error("snapshot custody record exceeds limit")]
    TooLarge,
    #[error("snapshot custody record could not be encoded")]
    Encode(#[source] serde_json::Error),
    #[error("invalid snapshot custody record")]
    Decode(#[source] serde_json::Error),
    #[error("snapshot custody identity mismatch")]
    IdentityMismatch,
    #[error("snapshot custody subject mismatch")]
    SubjectMismatch,
    #[error("snapshot custody record could not be created")]
    Create(#[from] kv::CreateError),
    #[error("snapshot custody record could not be updated")]
    Update(#[from] kv::UpdateError),
    #[error("snapshot custody record could not be read")]
    Read(#[from] kv::EntryError),
    #[error("snapshot custody stream is unavailable")]
    Stream(#[from] InfoError),
    #[error("snapshot custody message could not be read")]
    Message(#[from] RawMessageError),
}
