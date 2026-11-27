use std::time::Duration;

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct BackpressurePolicy {
    pub initial_delay: Duration,
    pub max_delay: Duration,
    /// Wall-clock time, from the first attempt on one chunk, before giving up.
    pub budget: Duration,
}

impl Default for BackpressurePolicy {
    fn default() -> Self {
        Self {
            initial_delay: Duration::from_millis(50),
            max_delay: Duration::from_secs(1),
            budget: Duration::from_secs(30),
        }
    }
}
