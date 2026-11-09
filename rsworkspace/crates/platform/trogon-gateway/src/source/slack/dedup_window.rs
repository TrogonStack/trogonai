use std::time::Duration;

use trogon_nats::jetstream::StreamMaxAge;

use super::constants::DEDUP_WINDOW;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DedupWindow(Duration);

impl DedupWindow {
    pub fn within(stream_max_age: StreamMaxAge) -> Self {
        match stream_max_age {
            StreamMaxAge::NoExpiry => Self(DEDUP_WINDOW),
            StreamMaxAge::ExpireAfter(max_age) => Self(DEDUP_WINDOW.min(max_age.into())),
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
