use oci_client::manifest::{OciImageManifest, OciManifest};
use olpc_cjson::CanonicalFormatter;
use serde::Serialize;
use sha2::{Digest as _, Sha256};
use trogon_sandbox_contract::Digest;

use crate::OciError;

pub(crate) fn sha256_digest(data: &[u8]) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(data)))
}

pub(crate) fn verify_sha256(data: &[u8], expected: &str) -> Result<(), OciError> {
    let actual = sha256_digest(data);
    if actual != expected {
        return Err(OciError::DigestMismatch {
            expected: expected.to_string(),
            actual,
        });
    }
    Ok(())
}

/// The digest of the canonical JSON form of an image manifest.
pub(crate) fn compute_manifest_digest(manifest: &OciImageManifest) -> Result<Digest, OciError> {
    let mut body = Vec::new();
    let mut serializer = serde_json::Serializer::with_formatter(&mut body, CanonicalFormatter::new());
    OciManifest::Image(manifest.clone())
        .serialize(&mut serializer)
        .map_err(|error| OciError::RegistryRejected(format!("manifest serialization failed: {error}")))?;
    Ok(Digest::new(sha256_digest(&body))?)
}
