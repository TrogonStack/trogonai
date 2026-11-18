#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PlanError {
    #[error("session plan contract is unsupported")]
    UnsupportedContract,
    #[error("session plan artifact digest does not match its original bytes")]
    DigestMismatch,
    #[error("session plan protobuf is malformed or ambiguous")]
    Wire,
    #[error("session plan exceeds its registered limits")]
    Limit,
    #[error("session plan contains an invalid value or inconsistent binding")]
    InvalidPlan,
    #[error("session plan does not match its authenticated original source")]
    OriginalSourceMismatch,
    #[error("session plan runtime contract is unsupported")]
    UnsupportedRuntime,
    #[error("session plan variable bindings do not match the original schema")]
    Variables,
    #[error("session original source is unavailable")]
    SourceUnavailable,
    #[error("session admission authority was withdrawn")]
    AuthorityWithdrawn,
}
