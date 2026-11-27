use trogon_sandbox_contract::FileResultTruncation;

/// Net difference between a baseline and now, from
/// [`SandboxClient::file_changes`](crate::SandboxClient::file_changes).
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct FileChanges {
    /// The baseline compared against: a checkpoint id or `"start"`.
    pub since: String,
    pub added: Vec<String>,
    pub modified: Vec<String>,
    pub deleted: Vec<String>,
    /// Whether the lists were cut at the worker's entry ceiling.
    pub truncated: FileResultTruncation,
}
