/// What signalling a detached command found.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum SignalOutcome {
    Signaled,
    AlreadyCompleted,
    NotFound,
}
