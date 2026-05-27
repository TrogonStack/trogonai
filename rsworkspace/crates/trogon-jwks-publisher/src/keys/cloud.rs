use async_trait::async_trait;

use super::{KeyError, KeySource};
use crate::jwks::Jwks;

/// AWS / GCP / Azure KMS-backed mesh signing keys.
///
/// Production adoption blocks on implementing public-key export and rotation
/// event subscription against the org's cloud KMS.
pub struct KmsKeySource;

impl KmsKeySource {
    pub fn new() -> Self {
        Self
    }
}

impl Default for KmsKeySource {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl KeySource for KmsKeySource {
    async fn current(&self) -> Result<Jwks, KeyError> {
        // TODO(prod): fetch active asymmetric key versions from cloud KMS and assemble JWKS.
        todo!("TODO(prod): implement KMS KeySource")
    }
}

/// HashiCorp Vault Transit-backed mesh signing keys.
///
/// Production adoption blocks on implementing Transit public-key export and
/// rotation watch against the org's Vault cluster.
pub struct VaultKeySource;

impl VaultKeySource {
    pub fn new() -> Self {
        Self
    }
}

impl Default for VaultKeySource {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl KeySource for VaultKeySource {
    async fn current(&self) -> Result<Jwks, KeyError> {
        // TODO(prod): read Transit key versions and assemble JWKS.
        todo!("TODO(prod): implement Vault KeySource")
    }
}
