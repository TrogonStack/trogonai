use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Decides when a terminal's output is due another activity signal.
///
/// The broker never sees terminal output, so a worker publishes an empty, throttled activity signal while output
/// flows, and a shell that only prints still counts toward the lease's idle timeout.
#[derive(Debug, Default)]
pub struct ActivitySignal {
    last: Mutex<Option<Instant>>,
}

impl ActivitySignal {
    /// Shortest gap between two activity signals for one terminal. Short enough that continuous output keeps the
    /// lease alive, without a signal per frame.
    pub const INTERVAL: Duration = Duration::from_secs(5);

    /// Whether a signal is due at `now`: true at most once per [`INTERVAL`](Self::INTERVAL), and it records `now`
    /// when it is.
    pub fn due(&self, now: Instant) -> bool {
        let mut last = self.last.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        match *last {
            Some(previous) if now.saturating_duration_since(previous) < Self::INTERVAL => false,
            _ => {
                *last = Some(now);
                true
            }
        }
    }
}
