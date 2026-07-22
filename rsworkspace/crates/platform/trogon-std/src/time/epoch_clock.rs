use std::time::SystemTime;

/// Wall-clock time source, for comparing against external timestamps
/// (e.g. Slack's `X-Slack-Request-Timestamp`).
///
/// Distinct from [`GetNow`](super::GetNow) which provides monotonic instants
/// for measuring elapsed durations.
pub trait EpochClock: Clone + Send + Sync + 'static {
    fn system_time(&self) -> SystemTime;
}
