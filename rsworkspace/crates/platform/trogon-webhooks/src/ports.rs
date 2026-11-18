use std::future::Future;

use axum::http::HeaderMap;
use trogon_std::SecretString;

use crate::{ConfigurationRevisionError, EndpointId, EndpointRegistrationError, RegisteredEndpoint};

#[derive(Debug, thiserror::Error)]
pub enum AuthenticationError {
    #[error("webhook authentication failed")]
    Unauthorized,
    #[error("webhook authentication is unavailable")]
    Unavailable,
    #[error("webhook authentication is unavailable")]
    UnavailableWithSource(#[source] Box<dyn std::error::Error + Send + Sync>),
}

#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    #[error("webhook registry is unavailable")]
    Unavailable,
    #[error("webhook registration is invalid")]
    Invalid,
    #[error("webhook registry read failed")]
    Read(#[source] async_nats::jetstream::kv::EntryError),
    #[error("webhook registry record cannot be decoded")]
    Decode(#[source] buffa::DecodeError),
    #[error("webhook registry record is invalid")]
    Registration(#[source] EndpointRegistrationError),
    #[error("webhook registry revision is invalid")]
    Revision(#[source] ConfigurationRevisionError),
    #[error("webhook registry record exceeds the size limit")]
    Oversized,
    #[error("webhook registry record names a different endpoint")]
    IdentityMismatch,
}

pub trait EndpointLookup: Clone + Send + Sync + 'static {
    fn lookup(
        &self,
        endpoint_id: &EndpointId,
    ) -> impl Future<Output = Result<Option<RegisteredEndpoint>, RegistryError>> + Send;
}

pub trait WebhookSecretLookup: Clone + Send + Sync + 'static {
    fn lookup_secret(
        &self,
        endpoint: &RegisteredEndpoint,
    ) -> impl Future<Output = Result<SecretString, AuthenticationError>> + Send;
}

pub trait WebhookVerifier: Clone + Send + Sync + 'static {
    fn verify(
        &self,
        endpoint: &RegisteredEndpoint,
        headers: &HeaderMap,
        body: &[u8],
    ) -> impl Future<Output = Result<(), AuthenticationError>> + Send;
}
