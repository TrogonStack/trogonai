/// A custody name was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum BoundedNameError {
    #[error("a name must not be empty")]
    Empty,
    #[error("a name is at most {max} bytes")]
    TooLong { max: usize },
    #[error("a name holds only ASCII letters, digits, '-', '_' and '.'")]
    InvalidCharacter,
}
