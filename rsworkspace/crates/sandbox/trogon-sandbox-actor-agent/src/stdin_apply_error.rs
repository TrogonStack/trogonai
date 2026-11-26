/// Why a stdin write did not reach the process.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum StdinApplyError {
    /// The write never started, so retrying the same `seq` is safe.
    Backpressure,
    /// The write may be partly applied or the writer is gone, so no `seq` is retryable.
    Broken,
}
