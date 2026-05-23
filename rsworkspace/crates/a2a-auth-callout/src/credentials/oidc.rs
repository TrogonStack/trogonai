// OIDC credential verifier — primary auth path.
// See docs/A2A_AUTH_CALLOUT_SKETCH.md §2 "Credential sources (preference order)".

use crate::error::AuthCalloutError;
use crate::jwt::UserJwtClaims;

/// Verifies an OIDC token and maps its claims to a `UserJwtClaims` ready to mint.
///
/// Phase 0 scaffold: the real implementation must validate the IdP token (introspection
/// or JWKS verification), map `sub` → tenant Account, and derive `caller_id` via a
/// tenant-scoped deterministic hash. Those paths remain `unimplemented!()`.
#[async_trait::async_trait]
pub trait OidcVerifier: Send + Sync + 'static {
    async fn verify(&self, token: &str) -> Result<UserJwtClaims, AuthCalloutError>;
}

pub struct StubOidcVerifier;

#[async_trait::async_trait]
impl OidcVerifier for StubOidcVerifier {
    async fn verify(&self, _token: &str) -> Result<UserJwtClaims, AuthCalloutError> {
        unimplemented!("OIDC token verification not implemented yet")
    }
}
