/// Why a [`crate::JwtSvidVerifier`] could not be constructed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum JwtVerifierError {
    #[error("expected audience is empty, too long or contains whitespace or control characters")]
    AudienceInvalid,
    #[error("expected audience is a bare trust domain ID")]
    AudienceIsTrustDomain,
    #[error("expected audience is not an absolute URI naming one specific service")]
    AudienceNotSpecific,
    #[error("no JWT bundles configured")]
    NoBundles,
    #[error("lifetime ceiling must be non-zero and at most one hour")]
    LifetimeCeilingInvalid,
    #[error("clock leeway must be at most one minute")]
    LeewayInvalid,
}
