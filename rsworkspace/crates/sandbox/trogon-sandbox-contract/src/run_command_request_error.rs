/// A command request does not satisfy the rules [`RunCommandRequest`](crate::RunCommandRequest) enforces.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub enum RunCommandRequestError {
    #[error("interactive stdin requires a detached command")]
    InteractiveStdinRequiresDetached,
    #[error("a lease deadline requires a detached command")]
    LeaseDeadlineRequiresDetached,
    #[error("a process spec requires a detached command")]
    ProcessRequiresDetached,
}
