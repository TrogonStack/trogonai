/// A string is not a KMS key identifier.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub enum KmsKeyIdError {
    #[error("value must not be empty")]
    Empty,
    #[error("value must not contain whitespace or control characters")]
    InvalidCharacter,
}
