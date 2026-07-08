//! Typed errors for the agent-side signer. No panics in library code: key
//! loading and signing failures surface here instead of via `expect`/`unwrap`.

/// Errors returned while constructing an [`crate::AgentSigner`] or signing a
/// NATS request.
#[derive(Debug, thiserror::Error)]
pub enum AgentSignerError {
    /// The supplied PEM did not decode into a PKCS#8 P-256 private key.
    #[error("invalid PKCS#8 PEM for P-256 signing key")]
    InvalidPkcs8(#[source] p256::pkcs8::Error),
    /// The public key's JWK could not be computed from the P-256 key.
    #[error("could not derive JWK from P-256 public key")]
    InvalidPublicKey,
    /// The public JWK failed RFC 7638 thumbprint computation.
    #[error("could not compute jkt for the agent's public key")]
    Thumbprint(#[source] trogon_aauth_verify::jkt::JktError),
}
