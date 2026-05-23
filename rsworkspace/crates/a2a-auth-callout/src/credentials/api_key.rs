// API key credential verifier — transitional path for clients migrating off legacy
// bearer flows. See docs/A2A_AUTH_CALLOUT_SKETCH.md §2 "Credential sources (preference order)".

use crate::error::AuthCalloutError;
use crate::jwt::UserJwtClaims;

/// Verifies a legacy API key and maps it to a `UserJwtClaims`.
///
/// Phase 0 scaffold: the real implementation must look up the key in the registry,
/// validate it, and derive `caller_id`. Those paths remain `unimplemented!()`.
#[deprecated(note = "transitional only; remove after OIDC migration")]
#[async_trait::async_trait]
pub trait ApiKeyVerifier: Send + Sync + 'static {
    async fn verify(&self, api_key: &str) -> Result<UserJwtClaims, AuthCalloutError>;
}

#[allow(deprecated)]
pub struct StubApiKeyVerifier;

#[allow(deprecated)]
#[async_trait::async_trait]
impl ApiKeyVerifier for StubApiKeyVerifier {
    async fn verify(&self, _api_key: &str) -> Result<UserJwtClaims, AuthCalloutError> {
        unimplemented!("API key verification not implemented yet")
    }
}
