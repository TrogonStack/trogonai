use trogon_sandbox_contract::FileWatchEvent;

/// One item of [`SandboxClient::watch_files`](crate::SandboxClient::watch_files).
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum FileWatchUpdate {
    Event(FileWatchEvent),
    /// Events were dropped from the worker's buffer before they were read; treat the watched tree as changed in
    /// ways the stream did not report.
    Overflowed,
}
