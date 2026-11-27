use std::num::TryFromIntError;

use super::captured_deadline_error::CapturedDeadlineError;
use super::logical_name_error::SnapshotLogicalNameError;
use super::reference_error::SnapshotReferenceError;

#[derive(Debug, Clone, thiserror::Error)]
pub enum SnapshotManifestError {
    #[error(transparent)]
    Reference(#[from] SnapshotReferenceError),
    #[error("snapshot metadata owner or version mismatch")]
    OwnerOrVersionMismatch,
    #[error("snapshot metadata identity mismatch")]
    IdentityMismatch,
    #[error("snapshot execution inventory exceeds its limit")]
    TooManyExecutions,
    #[error("snapshot immutable capture binding is missing")]
    MissingBinding,
    #[error("invalid opaque snapshot reference")]
    InvalidReferenceId,
    #[error("snapshot logical name is missing")]
    MissingLogicalName,
    #[error(transparent)]
    LogicalName(#[from] SnapshotLogicalNameError),
    #[error("legacy snapshot metadata has incompatible capture binding fields")]
    LegacyBindingFields,
    #[error("snapshot execution deadline provenance does not match its version")]
    DeadlineVersionMismatch,
    #[error(transparent)]
    Deadline(#[from] CapturedDeadlineError),
    #[error("invalid captured execution id")]
    InvalidExecutionId,
    #[error("duplicate captured execution id")]
    DuplicateExecutionId,
    #[error("invalid captured process identity")]
    InvalidProcessIdentity,
    #[error("captured output budget exceeds this worker")]
    OutputBudget(#[from] TryFromIntError),
    #[error(
        "legacy E2B snapshot lacks an immutable capture binding; recapture or explicitly verified migration is required"
    )]
    LegacyRestore,
}
