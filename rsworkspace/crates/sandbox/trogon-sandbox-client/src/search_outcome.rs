use trogon_sandbox_contract::{FileResultTruncation, SearchMatch};

/// Reply of [`SandboxClient::search`](crate::SandboxClient::search).
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct SearchOutcome {
    pub matches: Vec<SearchMatch>,
    pub truncated: FileResultTruncation,
    pub files_scanned: u64,
}
