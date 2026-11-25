/// A launch key does not satisfy the rules [`LaunchKey`](crate::LaunchKey) enforces.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub enum LaunchKeyError {
    #[error("launch key must not be empty")]
    Empty,
    #[error("launch key must be at most {max} characters")]
    TooLong { max: usize },
    #[error("launch key may only contain ASCII letters, digits, '_' and '-'")]
    InvalidCharacters,
}
