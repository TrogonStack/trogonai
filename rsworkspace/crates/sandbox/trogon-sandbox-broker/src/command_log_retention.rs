use std::time::Duration;

/// How much command output the log stream keeps.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct CommandLogRetention {
    /// Total bytes across all executions; the oldest chunks go first.
    pub max_bytes: i64,
    /// Longest a chunk is kept.
    pub max_age: Duration,
    /// Most chunks kept per execution, so one chatty command cannot push every
    /// other execution's output out of the stream.
    pub max_chunks_per_execution: i64,
    /// Longest an execution's exit record is kept. Should be at least
    /// `max_age`, so exit status outlives the output it closes.
    pub exit_max_age: Duration,
}

impl Default for CommandLogRetention {
    fn default() -> Self {
        Self {
            max_bytes: 1024 * 1024 * 1024,
            max_age: Duration::from_secs(24 * 60 * 60),
            max_chunks_per_execution: 16 * 1024,
            exit_max_age: Duration::from_secs(7 * 24 * 60 * 60),
        }
    }
}
