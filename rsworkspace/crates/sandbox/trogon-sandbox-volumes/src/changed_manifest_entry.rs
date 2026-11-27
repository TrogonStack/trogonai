use trogon_sandbox_contract::Digest;

use crate::ManifestKey;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangedManifestEntry {
    pub key: ManifestKey,
    pub digest: Digest,
    pub size_bytes: u64,
}
