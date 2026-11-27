use crate::{BackendError, KmsError};

/// A credential store could not seal, open, read or write a record.
#[derive(Debug, thiserror::Error)]
pub enum CredentialStoreError {
    #[error("credential not found")]
    NotFound,
    #[error("credential uses the legacy format and legacy reads are disabled")]
    LegacyDisabled,
    #[error("credential record is missing its envelope")]
    MissingEnvelope,
    #[error("credential belongs to a different tenant")]
    TenantMismatch,
    #[error("legacy credential has no recorded tenant; migrate it with a tenant")]
    LegacyTenantRequired,
    #[error("credential changed during re-encryption")]
    Conflict,
    #[error(transparent)]
    Kms(#[from] KmsError),
    #[error("credential record could not be encoded or decoded")]
    Codec(#[from] serde_json::Error),
    #[error(transparent)]
    Backend(#[from] BackendError),
}
