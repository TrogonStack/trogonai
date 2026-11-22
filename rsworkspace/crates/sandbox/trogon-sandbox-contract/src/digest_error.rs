/// A digest does not satisfy the SHA-256 wire grammar.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum DigestError {
    #[error("digest must start with sha256:")]
    MissingPrefix,
    #[error("digest hash must be lowercase hex")]
    InvalidHash,
}
