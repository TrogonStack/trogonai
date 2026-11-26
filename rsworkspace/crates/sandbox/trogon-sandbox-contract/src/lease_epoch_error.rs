/// A lease epoch does not satisfy the rules [`LeaseEpoch`](crate::LeaseEpoch) enforces.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub enum LeaseEpochError {
    #[error("lease epoch must be greater than zero")]
    Zero,
}
