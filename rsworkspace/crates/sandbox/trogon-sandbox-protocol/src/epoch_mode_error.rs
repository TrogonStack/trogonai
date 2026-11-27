/// A lease epoch fence mode name is not one of `off`, `warn`, or `require`.
#[derive(Debug, Clone, Eq, PartialEq, thiserror::Error)]
#[error("SANDBOX_FENCE_EPOCH must be off, warn or require, got {found}")]
pub struct EpochModeError {
    /// The rejected value, lowercased.
    pub found: String,
}
