//! A log line the browser transport asks its host to write.

/// How loudly a log line is written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SseLogLevel {
    /// Routine drops.
    Debug,
    /// Drops and overflows that point at a misbehaving peer or a lost request.
    Warn,
}

/// A log line and its level.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SseLogRecord {
    /// The level to write the line at.
    pub level: SseLogLevel,
    /// The line.
    pub text: String,
}
