/// A command readiness request does not satisfy the rules
/// [`CommandReadinessRequest`](crate::CommandReadinessRequest) enforces.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub enum CommandReadinessRequestError {
    #[error("command readiness lease id must be 1 to {max} bytes")]
    LeaseIdLength { max: usize },
    #[error(
        "command readiness lease id must be dot-separated tokens without whitespace, control characters, `*`, or `>`"
    )]
    LeaseIdSubject,
}
