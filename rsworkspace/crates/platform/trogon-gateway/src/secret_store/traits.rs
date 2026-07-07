use std::error::Error;
use std::future::Future;

use trogon_std::SecretString;

use super::{CredentialKind, CredentialMetadata, CredentialRef, CredentialScope, SecretMaterial};

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

pub trait SecretStore:
    SecretStorePut
    + SecretStoreGet<Error = <Self as SecretStorePut>::Error>
    + SecretStoreRotate<Error = <Self as SecretStorePut>::Error>
    + SecretStoreRevoke<Error = <Self as SecretStorePut>::Error>
    + SecretStoreMetadata<Error = <Self as SecretStorePut>::Error>
{
}

impl<T> SecretStore for T where
    T: SecretStorePut
        + SecretStoreGet<Error = <T as SecretStorePut>::Error>
        + SecretStoreRotate<Error = <T as SecretStorePut>::Error>
        + SecretStoreRevoke<Error = <T as SecretStorePut>::Error>
        + SecretStoreMetadata<Error = <T as SecretStorePut>::Error>
{
}
