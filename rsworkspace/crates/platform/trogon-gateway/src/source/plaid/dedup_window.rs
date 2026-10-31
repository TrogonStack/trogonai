use std::time::Duration;

use trogon_nats::jetstream::StreamMaxAge;
use trogon_std::NonZeroDuration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DedupWindow(Duration);

impl DedupWindow {
    pub fn spanning(timestamp_tolerance: NonZeroDuration, stream_max_age: StreamMaxAge) -> Self {
        let window = Duration::from(timestamp_tolerance).saturating_mul(2);
        match stream_max_age {
            StreamMaxAge::NoExpiry => Self(window),
            StreamMaxAge::ExpireAfter(max_age) => Self(window.min(max_age.into())),
        }
    }
}

impl From<DedupWindow> for Duration {
    fn from(window: DedupWindow) -> Self {
        window.0
    }
}

#[cfg(test)]
mod tests;
