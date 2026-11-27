/// A string is not a storage resource name.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub enum StoreResourceNameError {
    #[error("value must not be empty")]
    Empty,
    #[error("value must not contain whitespace or control characters")]
    InvalidCharacter,
}
