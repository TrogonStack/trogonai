use sha2::{Digest as _, Sha256};
use trogon_sandbox_contract::Digest;

use crate::digest_hash::digest_from_hash;
use crate::{ChangedManifestEntry, VolumeError};

/// The files a push uploads, sorted by key when built from a directory.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChangedManifest {
    pub entries: Vec<ChangedManifestEntry>,
}

impl ChangedManifest {
    /// A digest over every entry's key and digest, in entry order.
    pub fn digest(&self) -> Result<Digest, VolumeError> {
        let mut hasher = Sha256::new();
        for entry in &self.entries {
            hasher.update(entry.key.as_str().as_bytes());
            hasher.update(entry.digest.as_str().as_bytes());
        }
        digest_from_hash(&hasher.finalize())
    }
}
