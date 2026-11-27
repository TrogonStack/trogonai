/// A key hash does not satisfy the rules [`ApiKeyHash`](crate::ApiKeyHash) enforces.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub enum ApiKeyHashError {
    #[error("API key hash must be 64 characters")]
    WrongLength,
    #[error("API key hash must contain only lowercase hexadecimal characters")]
    InvalidCharacter,
}
