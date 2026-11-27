/// Why a stdin write did not reach the process.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub(crate) enum StdinApplyError {
    /// The write never started, so retrying the same `seq` is safe.
    #[error("stdin is busy; retry the same sequence number")]
    Backpressure,
    /// Stdin can no longer be trusted: the write may be partly applied or
    /// the writer is gone, so no `seq` is retryable.
    #[error("stdin is broken")]
    Broken,
}
