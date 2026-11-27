use sha2::{Digest, Sha256};
use trogon_sandbox_contract::{FileSha256, FileSha256Error};

pub(crate) fn encode_digest(digest: &[u8]) -> Result<FileSha256, FileSha256Error> {
    FileSha256::new(digest.iter().map(|byte| format!("{byte:02x}")).collect::<String>())
}

/// The SHA-256 of `bytes`, the content hash change tracking compares.
pub fn content_hash(bytes: &[u8]) -> Result<FileSha256, FileSha256Error> {
    encode_digest(&Sha256::digest(bytes))
}
