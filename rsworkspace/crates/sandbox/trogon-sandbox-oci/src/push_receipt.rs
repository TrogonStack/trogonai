use trogon_sandbox_contract::{Digest, OciArtifactRef};

/// The artifact a push stored and the digest of its manifest.
#[derive(Debug, Clone)]
pub struct PushReceipt {
    pub reference: OciArtifactRef,
    pub manifest_digest: Digest,
}
