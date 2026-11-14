use axum::http::HeaderMap;

use crate::{AuthenticationError, RegisteredEndpoint, WebhookSecretLookup, WebhookVerifier};

#[derive(Clone)]
pub struct SecretWebhookVerifier<L> {
    lookup: L,
}

impl<L> SecretWebhookVerifier<L> {
    pub fn new(lookup: L) -> Self {
        Self { lookup }
    }
    pub fn lookup(&self) -> &L {
        &self.lookup
    }
}

impl<L: WebhookSecretLookup> WebhookVerifier for SecretWebhookVerifier<L> {
    async fn verify(
        &self,
        _endpoint: &RegisteredEndpoint,
        _headers: &HeaderMap,
        _body: &[u8],
    ) -> Result<(), AuthenticationError> {
        Err(AuthenticationError::Unauthorized)
    }
}
