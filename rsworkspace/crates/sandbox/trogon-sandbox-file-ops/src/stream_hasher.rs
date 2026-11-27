use sha2::{Digest, Sha256};
use trogon_sandbox_contract::{FileSha256, FileSha256Error};

use crate::content_hash::encode_digest;

/// Incremental SHA-256 for a worker that reaches file content in pieces.
#[derive(Default)]
pub struct StreamHasher(Sha256);

impl StreamHasher {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn update(&mut self, bytes: &[u8]) {
        self.0.update(bytes);
    }

    /// The digest of everything fed so far.
    pub fn finish(self) -> Result<FileSha256, FileSha256Error> {
        encode_digest(&self.0.finalize())
    }
}
