use std::time::Duration;

/// An internal failure. It always refuses in `Enforce`: a decision that could
/// not be made is not a decision to admit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, thiserror::Error)]
#[non_exhaustive]
pub enum EvaluatorError {
    /// The snapshot claims to have been vouched for after `now`. Either the
    /// clock moved backward or the stamp is wrong, and in both cases its age
    /// cannot be trusted.
    #[error("policy snapshot is stamped {by:?} in the future")]
    SnapshotFromTheFuture {
        /// How far ahead of `now` the stamp is.
        by: Duration,
    },
}
