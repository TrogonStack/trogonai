/// An authority mode name is not one of `off`, `warn`, or `enforce`.
#[derive(Debug, Clone, Eq, PartialEq, thiserror::Error)]
#[error("SANDBOX_WORKER_AUTHORITY must be off, warn or enforce, got {found}")]
pub struct AuthorityModeError {
    /// The rejected value, lowercased.
    pub found: String,
}
