use std::num::NonZeroUsize;

/// Why the session table refused a session.
#[derive(Debug, Clone, Eq, PartialEq, thiserror::Error)]
pub(crate) enum SessionRefusalError {
    #[error("worker is at its limit of {max} ssh sessions")]
    AtCapacity { max: NonZeroUsize },
    #[error("ssh session belongs to another lease")]
    BoundToAnotherLease,
}
