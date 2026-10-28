use std::error::Error;
use std::future::Future;

use trogon_std::SecretString;

use super::{SecretDestroyReason, SecretMaterial};
use crate::credential::commands::domain::{CredentialKind, CredentialMetadata, CredentialRef, CredentialScope};

pub trait SecretStorePut: Send + Sync + Clone + 'static {
    type Error: Error + Send + Sync;

    fn put(
        &self,
        scope: CredentialScope,
        kind: CredentialKind,
        value: SecretString,
    ) -> impl Future<Output = Result<CredentialRef, Self::Error>> + Send;
}

pub trait SecretStoreGet: Send + Sync + Clone + 'static {
    type Error: Error + Send + Sync;

    fn get(&self, credential: &CredentialRef) -> impl Future<Output = Result<SecretMaterial, Self::Error>> + Send;
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
