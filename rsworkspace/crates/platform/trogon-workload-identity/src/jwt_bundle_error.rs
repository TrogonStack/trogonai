/// Why a JWT bundle could not be built. Carries no key material.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum JwtBundleError {
    #[error("JWKS document is too large")]
    TooLarge,
    #[error("JWKS document is not valid JSON of the expected shape")]
    Malformed,
    #[error("JWKS document has no JWT-SVID signing keys")]
    NoAuthorities,
    #[error("JWKS document has too many JWT-SVID signing keys")]
    TooManyAuthorities,
    #[error("a JWT-SVID signing key has no kid")]
    MissingKeyId,
    #[error("two JWT-SVID signing keys share a kid")]
    DuplicateKeyId,
    #[error("a JWT-SVID signing key has an unsupported type, curve or size")]
    UnsupportedKey,
    #[error("a bundle for this trust domain is already present")]
    DuplicateTrustDomain,
    #[error("the update carries no bundles")]
    NoBundles,
    #[error("the update carries too many bundles")]
    TooManyBundles,
    #[error("the update names a trust domain that cannot be parsed")]
    InvalidTrustDomain,
}
