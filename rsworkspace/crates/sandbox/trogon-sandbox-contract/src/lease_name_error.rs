/// A lease name does not satisfy the wire rules.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub enum LeaseNameError {
    #[error("lease name must not be empty")]
    Empty,
    #[error("lease name must be at most {max} bytes")]
    TooLong { max: usize },
}
