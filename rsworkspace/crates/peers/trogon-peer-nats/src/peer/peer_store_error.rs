use trogon_decider_nats::{EnsureBucketError, EnsureStreamError};

use super::{PeerSnapshotConfigMismatchError, PeerStreamConfigMismatchError};

#[derive(Debug, thiserror::Error)]
pub enum PeerStoreError {
    #[error("peer event stream provisioning failed: {0}")]
    Stream(#[from] EnsureStreamError),
    #[error("peer snapshot bucket provisioning failed: {0}")]
    Bucket(#[from] EnsureBucketError),
    #[error("peer event stream has unsafe existing configuration: {0}")]
    UnsafeConfig(#[from] PeerStreamConfigMismatchError),
    #[error("peer snapshot bucket has unsafe existing configuration: {0}")]
    UnsafeSnapshotConfig(#[from] PeerSnapshotConfigMismatchError),
    #[error("peer event subject configuration failed: {0}")]
    Subject(#[from] super::PeerSubjectError),
}
