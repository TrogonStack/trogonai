use std::time::Duration;

/// How long the supervisor waits before relaunching an agent that exited.
///
/// The delay doubles after each short-lived run, up to `max`, so a child that
/// crashes on start (a bad command, a missing credential) does not spin. A run
/// that stayed up at least `stable_after` counts as healthy, and the next
/// delay starts again from `initial`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RestartPolicy {
    initial: Duration,
    max: Duration,
    stable_after: Duration,
}

impl RestartPolicy {
    pub const DEFAULT: Self = Self {
        initial: Duration::from_secs(1),
        max: Duration::from_secs(30),
        stable_after: Duration::from_secs(60),
    };

    /// The delay before the next launch, given the delay used before the run
    /// that just ended and how long that run lasted.
    pub fn next_delay(&self, previous: Option<Duration>, uptime: Duration) -> Duration {
        match previous {
            Some(previous) if uptime < self.stable_after => previous.saturating_mul(2).min(self.max),
            _ => self.initial,
        }
    }
}

impl Default for RestartPolicy {
    fn default() -> Self {
        Self::DEFAULT
    }
}

#[cfg(test)]
mod tests;
