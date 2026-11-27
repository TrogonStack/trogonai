use std::time::Duration;

/// How long stdin writes may wait before the caller is told something.
#[derive(Debug, Clone, Copy)]
pub(crate) struct StdinDeadlines {
    /// How long a write waits for the process to accept it before the caller is told to retry with the same sequence
    /// number.
    pub(crate) accept: Duration,
    /// How long a write the process already started may take to finish before stdin is declared broken.
    pub(crate) started: Duration,
}

impl Default for StdinDeadlines {
    fn default() -> Self {
        Self {
            accept: Duration::from_secs(5),
            started: Duration::from_secs(30),
        }
    }
}
