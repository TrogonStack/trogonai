use thiserror::Error;
use trogon_sandbox_contract::DigestError;

/// An environment definition could not be turned into a digest.
#[derive(Debug, Error)]
pub enum EnvironmentDigestError {
    /// The definition could not be encoded.
    #[error("environment definition could not be encoded: {0}")]
    Encode(#[from] serde_json::Error),
    /// The computed hash did not render as a valid digest.
    #[error("environment digest is malformed: {0}")]
    Malformed(#[from] DigestError),
}
