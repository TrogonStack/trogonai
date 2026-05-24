use crate::error::AuthCalloutError;

/// Placeholder for operator vault custody. Real integration will live behind a feature flag.
#[derive(Debug)]
pub struct VaultSigningKeySource;

impl VaultSigningKeySource {
    pub fn load() -> Result<Self, AuthCalloutError> {
        Err(AuthCalloutError::Internal(
            "vault source not implemented".into(),
        ))
    }
}
