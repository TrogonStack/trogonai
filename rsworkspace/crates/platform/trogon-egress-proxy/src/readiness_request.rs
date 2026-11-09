#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReadinessRequest {
    Probe,
    NotFound,
    Malformed,
    TooLarge,
    TimedOut,
    Closed,
}
