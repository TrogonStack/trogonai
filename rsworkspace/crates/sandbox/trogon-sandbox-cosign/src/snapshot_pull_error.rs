use trogon_sandbox_contract::{Digest, LeaseError};

use crate::CosignError;

/// Why a snapshot pull was refused after verification.
#[derive(Debug, thiserror::Error)]
pub enum SnapshotPullError {
    /// The signature, or the signed annotations, did not verify; the broker-visible lease error.
    #[error(transparent)]
    SignatureRejected(LeaseError),
    /// Verification could not run to completion.
    #[error(transparent)]
    Cosign(CosignError),
}

/// The broker-visible [`LeaseError`] for a snapshot whose signature did not verify.
pub fn snapshot_signature_lease_error(digest: Digest) -> LeaseError {
    LeaseError::SnapshotSignatureInvalid { digest }
}

/// Convert verification errors into worker-facing pull errors.
pub fn into_pull_error(error: CosignError) -> SnapshotPullError {
    match error {
        CosignError::SignatureInvalid { digest } | CosignError::AnnotationMismatch { digest } => {
            SnapshotPullError::SignatureRejected(snapshot_signature_lease_error(digest))
        }
        other => SnapshotPullError::Cosign(other),
    }
}
