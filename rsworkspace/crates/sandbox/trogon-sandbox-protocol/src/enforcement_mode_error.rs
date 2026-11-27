/// A dispatch verification mode name is not one of `off`, `warn`, or `require`.
#[derive(Debug, Clone, Eq, PartialEq, thiserror::Error)]
#[error("SANDBOX_DISPATCH_VERIFY must be off, warn or require, got {found}")]
pub struct EnforcementModeError {
    /// The rejected value, lowercased.
    pub found: String,
}
