use trogon_credential_aggregate::CredentialProtoDecodeError;
use trogon_credential_domain::{CredentialKey, CredentialRef};
use trogon_secret_store::SecretStoreError;

use super::{CredentialReleaseError, CredentialStateReadError, ReleaseAuditError, SealError};

/// A release that could not complete, for the logs only.
///
/// Unlike a denial it is not a decision about the requester, so it is never
/// audited as one; the caller learns only the category.
#[derive(Debug, PartialEq, thiserror::Error)]
pub(crate) enum ReleaseFaultError {
    #[error("{0}")]
    StateRead(CredentialStateReadError),
    #[error("credential state does not decode: {0}")]
    StateUndecodable(CredentialProtoDecodeError),
    #[error("credential state names {found}, not the requested credential")]
    OtherCredential { found: CredentialKey },
    #[error("secret store lacks the releasable version of {credential}")]
    StoreMissing { credential: CredentialRef },
    #[error("{0}")]
    StoreUnavailable(SecretStoreError),
    #[error("{0}")]
    AuditUnavailable(ReleaseAuditError),
    #[error("{0}")]
    SealFailed(SealError),
}

impl ReleaseFaultError {
    /// A stable name for the fault that log filters and alerts key on.
    pub(crate) fn code(&self) -> &'static str {
        match self {
            Self::StateRead(CredentialStateReadError::Unavailable { .. }) => "state_unavailable",
            Self::StateRead(CredentialStateReadError::Undecodable { .. }) => "stream_undecodable",
            Self::StateUndecodable(_) => "state_undecodable",
            Self::OtherCredential { .. } => "other_credential",
            Self::StoreMissing { .. } => "store_missing",
            Self::StoreUnavailable(_) => "store_unavailable",
            Self::AuditUnavailable(_) => "audit_unavailable",
            Self::SealFailed(_) => "seal_failed",
        }
    }

    /// What the requester is told.
    ///
    /// A stream or store that disagrees with itself is an integrity failure:
    /// the tenant and policy checks already passed, so saying so reveals
    /// nothing, and retrying would not help.
    pub(crate) fn failure(&self) -> CredentialReleaseError {
        match self {
            Self::StateRead(CredentialStateReadError::Unavailable { .. })
            | Self::StoreUnavailable(_)
            | Self::AuditUnavailable(_) => CredentialReleaseError::Unavailable,
            Self::StateRead(CredentialStateReadError::Undecodable { .. })
            | Self::StateUndecodable(_)
            | Self::OtherCredential { .. }
            | Self::StoreMissing { .. } => CredentialReleaseError::IntegrityFailure,
            Self::SealFailed(_) => CredentialReleaseError::Internal,
        }
    }
}
