use async_trait::async_trait;
use trogon_sandbox_contract::{CredentialId, CredentialProvider, PullSecretId, TenantId};

use crate::{CredentialMaterial, CredentialStoreError, CredentialSummary};

/// Where provider credentials live, sealed at rest.
#[async_trait]
pub trait CredentialStore: Send + Sync {
    /// Stores the credential as version 1 and returns the version.
    async fn put_credential(
        &self,
        provider: CredentialProvider,
        tenant: &TenantId,
        credential_id: &CredentialId,
        material: &CredentialMaterial,
    ) -> Result<u32, CredentialStoreError>;

    async fn get_material(
        &self,
        provider: CredentialProvider,
        tenant: &TenantId,
        credential_id: &CredentialId,
    ) -> Result<CredentialMaterial, CredentialStoreError>;

    /// Replaces the material and returns the new version.
    async fn rotate_credential(
        &self,
        provider: CredentialProvider,
        tenant: &TenantId,
        credential_id: &CredentialId,
        material: &CredentialMaterial,
    ) -> Result<u32, CredentialStoreError>;

    /// Rewrites the record in the current envelope format without changing its material, returning the new
    /// version. This is the migration primitive.
    async fn reencrypt_credential(
        &self,
        provider: CredentialProvider,
        tenant: Option<&TenantId>,
        credential_id: &CredentialId,
    ) -> Result<u32, CredentialStoreError>;

    /// Ids of records still in the legacy format.
    async fn list_legacy(&self, provider: CredentialProvider) -> Result<Vec<CredentialId>, CredentialStoreError>;

    async fn map_pull_secret(
        &self,
        provider: CredentialProvider,
        pull_secret_id: &PullSecretId,
        credential_id: &CredentialId,
    ) -> Result<(), CredentialStoreError>;

    /// The credential a pull secret maps to, or the credential with the same id when it has no mapping.
    async fn resolve_pull_secret(
        &self,
        provider: CredentialProvider,
        pull_secret_id: &PullSecretId,
    ) -> Result<CredentialId, CredentialStoreError>;

    async fn list_credentials(
        &self,
        provider: CredentialProvider,
    ) -> Result<Vec<CredentialSummary>, CredentialStoreError>;
}
