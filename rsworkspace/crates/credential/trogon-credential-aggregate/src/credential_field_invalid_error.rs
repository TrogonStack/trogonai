use trogon_credential_domain::{
    CredentialDestroyReasonError, CredentialFailureReasonError, CredentialFingerprintError, CredentialIdError,
    CredentialOwnerIdError, CredentialVersionError, SourceIntegrationIdError,
};

use crate::UnspecifiedEnumKind;

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum CredentialFieldInvalidError {
    #[error(transparent)]
    CredentialId(#[from] CredentialIdError),
    #[error(transparent)]
    OwnerId(#[from] CredentialOwnerIdError),
    #[error(transparent)]
    Fingerprint(#[from] CredentialFingerprintError),
    #[error(transparent)]
    FailureReason(#[from] CredentialFailureReasonError),
    #[error(transparent)]
    DestroyReason(#[from] CredentialDestroyReasonError),
    #[error(transparent)]
    Version(#[from] CredentialVersionError),
    #[error(transparent)]
    SourceIntegrationId(#[from] SourceIntegrationIdError),
    #[error("credential scope key must not be empty")]
    EmptyScopeKey,
    #[error("scope key '{scope_key}' does not match source '{source_kind}'")]
    ScopeKeyMismatch {
        scope_key: String,
        source_kind: &'static str,
    },
    #[error("unspecified {0}")]
    UnspecifiedEnum(UnspecifiedEnumKind),
    #[error("unknown enum value {0}")]
    UnknownEnumValue(i32),
}
