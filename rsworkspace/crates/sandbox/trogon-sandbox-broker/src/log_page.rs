use trogon_sandbox_contract::LogChunk;

/// One page of retained output.
#[derive(Debug, Clone, Eq, PartialEq, Default)]
pub struct LogPage {
    pub chunks: Vec<LogChunk>,
    /// `after_seq` for the following page.
    pub next_seq: u64,
    pub has_more: bool,
    pub truncated: bool,
    /// The worker published the execution's exit marker.
    pub exited: bool,
}
