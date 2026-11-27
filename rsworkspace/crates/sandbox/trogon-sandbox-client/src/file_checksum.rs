use trogon_sandbox_contract::FileSha256;

/// SHA-256 of a byte range of a file in the sandbox.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct FileChecksum {
    /// Lowercase hex digest.
    pub sha256: FileSha256,
    /// Bytes the digest covers.
    pub bytes: u64,
    /// Size of the whole file.
    pub total_size: u64,
}
