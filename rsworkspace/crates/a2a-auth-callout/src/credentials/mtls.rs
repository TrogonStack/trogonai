// mTLS credential verifier — service-to-service auth path.
// See docs/A2A_AUTH_CALLOUT_SKETCH.md §2 "Credential sources (preference order)".

use crate::error::AuthCalloutError;
use crate::jwt::UserJwtClaims;

/// Verifies an mTLS certificate subject/SAN and maps it to a `UserJwtClaims`.
///
/// Phase 0 scaffold: the real implementation must extract the certificate subject
/// from the `client_info` callout field, validate it against the trusted CA store,
/// and derive `caller_id`. Those paths remain `unimplemented!()`.
#[async_trait::async_trait]
pub trait MTlsVerifier: Send + Sync + 'static {
    async fn verify(&self, cert_subject: &str) -> Result<UserJwtClaims, AuthCalloutError>;
}

pub struct StubMTlsVerifier;

#[async_trait::async_trait]
impl MTlsVerifier for StubMTlsVerifier {
    async fn verify(&self, _cert_subject: &str) -> Result<UserJwtClaims, AuthCalloutError> {
        unimplemented!("mTLS certificate verification not implemented yet")
    }
}
