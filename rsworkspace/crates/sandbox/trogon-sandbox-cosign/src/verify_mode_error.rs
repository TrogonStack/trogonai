/// A verify mode name was not `off`, `warn` or `require`.
#[derive(Debug, Clone, Eq, PartialEq, thiserror::Error)]
#[error("unknown cosign verify mode '{0}' (expected off, warn or require)")]
pub struct VerifyModeError(pub String);
