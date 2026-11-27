use async_nats::jetstream::kv;

use crate::custody_bucket_error::CustodyBucketError;

use super::manifest_error::SnapshotManifestError;
use super::reference_error::SnapshotReferenceError;

#[derive(Debug, thiserror::Error)]
pub enum SnapshotManifestsError {
    #[error("snapshot metadata requires worker JetStream context")]
    NotConfigured,
    #[error("snapshot metadata context is already configured")]
    AlreadyConfigured,
    #[error("snapshot metadata bucket is unavailable")]
    Bucket(#[from] CustodyBucketError),
    #[error(transparent)]
    Reference(#[from] SnapshotReferenceError),
    #[error(transparent)]
    Manifest(#[from] SnapshotManifestError),
    #[error("snapshot metadata exceeds the byte limit")]
    TooLarge,
    #[error("snapshot metadata could not be encoded")]
    Encode(#[source] serde_json::Error),
    #[error("invalid snapshot metadata")]
    Decode(#[source] serde_json::Error),
    #[error("snapshot metadata could not be created")]
    Create(#[from] kv::CreateError),
    #[error("snapshot metadata could not be read")]
    Read(#[from] kv::EntryError),
    #[error("snapshot metadata could not be deleted")]
    Delete(#[from] kv::DeleteError),
}
