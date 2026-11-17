use trogon_secret_store::SecretStoreError;

use super::DenialReason;
use super::release_fault_error::ReleaseFaultError;

/// Why the release core stopped short of a release.
#[derive(Debug, PartialEq)]
pub(crate) enum Refusal {
    Denied(DenialReason),
    Faulted(ReleaseFaultError),
}

impl From<DenialReason> for Refusal {
    fn from(reason: DenialReason) -> Self {
        Self::Denied(reason)
    }
}

impl From<ReleaseFaultError> for Refusal {
    fn from(fault: ReleaseFaultError) -> Self {
        Self::Faulted(fault)
    }
}

/// Mirrors how the gateway answers the same store errors: a version the store
/// no longer reads is a denial, as a revocation racing the fold would be, while
/// a version the store never had means the stream and the store disagree.
impl From<SecretStoreError> for Refusal {
    fn from(error: SecretStoreError) -> Self {
        match error {
            SecretStoreError::Unreadable { .. } => Self::Denied(DenialReason::VersionUnreadable),
            SecretStoreError::Missing { credential } => Self::Faulted(ReleaseFaultError::StoreMissing { credential }),
            error @ (SecretStoreError::BackendUnavailable { .. }
            | SecretStoreError::Unwritable { .. }
            | SecretStoreError::InvalidGenerated { .. }) => Self::Faulted(ReleaseFaultError::StoreUnavailable(error)),
        }
    }
}
