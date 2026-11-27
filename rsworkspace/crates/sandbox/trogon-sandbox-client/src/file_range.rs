use trogon_sandbox_contract::{FileReadEnd, FileResultTruncation};

/// Reply of [`SandboxClient::read_range`](crate::SandboxClient::read_range).
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct FileRange {
    pub data: Vec<u8>,
    pub offset: u64,
    pub total_size: u64,
    pub eof: FileReadEnd,
    pub truncated: FileResultTruncation,
}
