/// Why audit could not durably take a record.
///
/// One vocabulary serves the refusal counter, the drop counter, the structured
/// log line and the error a surface maps to its own unavailable code. A full
/// stream and an unreachable one are separate reasons so they page differently
/// and refuse identically.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum FailureReason {
    /// The stream could not be reached or refused the publish.
    Unreachable,
    /// No acknowledgement arrived, after the bounded same-id retries.
    Timeout,
    /// The stream is full and is configured with `discard: new`.
    StreamFull,
    /// The deny retry buffer is full, so the writer is audit-degraded.
    Degraded,
    /// The best-effort queue is full.
    QueueFull,
    /// A deny could not be buffered because the retry buffer is full.
    BufferFull,
}

impl FailureReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unreachable => "unreachable",
            Self::Timeout => "timeout",
            Self::StreamFull => "stream_full",
            Self::Degraded => "degraded",
            Self::QueueFull => "queue_full",
            Self::BufferFull => "buffer_full",
        }
    }
}

impl std::fmt::Display for FailureReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests;
