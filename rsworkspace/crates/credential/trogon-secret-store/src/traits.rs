use std::error::Error;
use std::future::Future;

use trogon_credential_domain::{AuditCorrelationId, CredentialIdentity, CredentialMetadata, CredentialRef};
use trogon_std::SecretString;

use super::{SecretDestroyReason, SecretMaterial};

pub trait SecretStorePut: Send + Sync + Clone + 'static {
    type Error: Error + Send + Sync;

    fn put(
        &self,
        identity: CredentialIdentity,
        value: SecretString,
    ) -> impl Future<Output = Result<CredentialRef, Self::Error>> + Send;
}

pub trait SecretStoreGet: Send + Sync + Clone + 'static {
    type Error: Error + Send + Sync;

    fn get(&self, credential: &CredentialRef) -> impl Future<Output = Result<SecretMaterial, Self::Error>> + Send;
}

/// Reads a credential on behalf of one audited release.
///
/// The correlation id joins the release's own audit record to the one the
/// backing store writes for the read, per ADR#0023 Decision 6, so an adapter
/// passes it to the store rather than dropping it.
pub trait SecretStoreCorrelatedGet: Send + Sync + Clone + 'static {
    type Error: Error + Send + Sync;

    fn get_correlated(
        &self,
        credential: &CredentialRef,
        correlation_id: &AuditCorrelationId,
    ) -> impl Future<Output = Result<SecretMaterial, Self::Error>> + Send;
}

pub trait SecretStoreRotate: Send + Sync + Clone + 'static {
    type Error: Error + Send + Sync;

    fn rotate(
        &self,
        credential: &CredentialRef,
        value: SecretString,
    ) -> impl Future<Output = Result<CredentialRef, Self::Error>> + Send;
}

pub trait SecretStoreRevoke: Send + Sync + Clone + 'static {
    type Error: Error + Send + Sync;

    fn revoke(&self, credential: &CredentialRef) -> impl Future<Output = Result<(), Self::Error>> + Send;
}

pub trait SecretStoreMetadata: Send + Sync + Clone + 'static {
    type Error: Error + Send + Sync;

    fn metadata(
        &self,
        credential: &CredentialRef,
    ) -> impl Future<Output = Result<CredentialMetadata, Self::Error>> + Send;
}

pub trait SecretStoreDestroy: Send + Sync + Clone + 'static {
    type Error: Error + Send + Sync;

    fn destroy(
        &self,
        credential: &CredentialRef,
        reason: &SecretDestroyReason,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;
}
