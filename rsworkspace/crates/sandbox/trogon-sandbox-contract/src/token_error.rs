/// A sandbox identifier does not satisfy the wire token grammar.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub enum TokenError {
    #[error("identifier must not be empty")]
    Empty,
    #[error("identifier is {actual} bytes; maximum is {maximum}")]
    TooLong { actual: usize, maximum: usize },
    #[error("identifier contains a byte outside ASCII letters, digits, '-' or '_' at index {index}")]
    InvalidCharacter { index: usize },
}
