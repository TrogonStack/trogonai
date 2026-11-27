use std::time::Duration;

/// What one timed step produced over every iteration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LatencyStep {
    pub step: &'static str,
    pub samples: Vec<Duration>,
    pub failures: usize,
    /// Why the step has no samples because it did not run, such as a worker that answers `Unsupported`.
    pub skipped: Option<String>,
}

impl LatencyStep {
    pub fn new(step: &'static str) -> Self {
        Self {
            step,
            samples: Vec::new(),
            failures: 0,
            skipped: None,
        }
    }

    /// Marks the step as not run. The first reason given is kept.
    pub fn skip(&mut self, why: impl Into<String>) {
        self.skipped.get_or_insert_with(|| why.into());
    }
}
