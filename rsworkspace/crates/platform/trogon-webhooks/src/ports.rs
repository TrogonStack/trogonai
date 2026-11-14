use std::future::Future;

use axum::http::HeaderMap;
use trogon_std::SecretString;

use crate::{EndpointId, RegisteredEndpoint};

#[derive(Debug, thiserror::Error)]
pub enum AuthenticationError {
    #[error("webhook authentication failed")]
    Unauthorized,
    #[error("webhook authentication is unavailable")]
    Unavailable,
}

#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    #[error("webhook registry is unavailable")]
    Unavailable,
    #[error("webhook registration is invalid")]
    Invalid,
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
