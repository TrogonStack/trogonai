/// A lease request key does not satisfy the rules [`LeaseRequestKey`](crate::LeaseRequestKey) enforces.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub enum LeaseRequestKeyError {
    #[error("lease request key must not be empty")]
    Empty,
    #[error("lease request key must be at most {max} bytes")]
    TooLong { max: usize },
    #[error("lease request key contains a byte outside ASCII letters, digits, '-' or '_' at index {index}")]
    InvalidCharacter { index: usize },
}
