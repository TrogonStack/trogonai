use crate::api::ApiError;
use crate::snapshot_custody::{SnapshotClockError, SnapshotCustodyError};
use crate::snapshots::{SnapshotManifestError, SnapshotManifestsError, SnapshotReferenceError};

#[derive(Debug, thiserror::Error)]
pub enum SnapshotLifecycleError {
    #[error(transparent)]
    Custody(#[from] SnapshotCustodyError),
    #[error(transparent)]
    Manifests(#[from] SnapshotManifestsError),
    #[error(transparent)]
    Manifest(#[from] SnapshotManifestError),
    #[error(transparent)]
    Reference(#[from] SnapshotReferenceError),
    #[error(transparent)]
    Clock(#[from] SnapshotClockError),
    #[error(transparent)]
    Api(#[from] ApiError),
    #[error("snapshot metadata identity already exists")]
    MetadataExists,
    #[error("snapshot custody is missing")]
    CustodyMissing,
    #[error("snapshot cleanup already started")]
    CleanupStarted,
    #[error("snapshot delete was acknowledged but exact absence is not confirmed")]
    AbsenceUnconfirmed,
    #[error("snapshot cleanup identity mismatch")]
    CleanupIdentityMismatch,
    #[error("snapshot cleanup custody could not be claimed after bounded retries")]
    ClaimExhausted,
    #[error("snapshot is not pending cleanup")]
    NotPendingCleanup,
    #[error("cleanup lacks canonical template identity")]
    CleanupTemplateMissing,
    #[error("snapshot cleanup is already in progress")]
    CleanupInProgress,
    #[error("capture recovery binding mismatch")]
    RecoveryBindingMismatch,
    #[error("snapshot cleanup custody persistence failed: {custody}; immediate exact cleanup: {cleanup:?}")]
    CustodyPersistence {
        custody: Box<SnapshotLifecycleError>,
        cleanup: Result<(), Box<SnapshotLifecycleError>>,
    },
}
