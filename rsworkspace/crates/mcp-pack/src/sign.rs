use nkeys::KeyPair;
use trogon_mcp_gateway::bundle::{manifest_digest_bytes, signature_path};

use crate::McpPackError;

/// Detached Ed25519 signature over the SHA-256 digest of canonical `manifest.toml` bytes.
pub fn sign_manifest(signer: &KeyPair, manifest_bytes: &[u8]) -> Result<Vec<u8>, McpPackError> {
    signer
        .sign(&manifest_digest_bytes(manifest_bytes))
        .map_err(|error| McpPackError::Sign(error.to_string()))
}

pub fn signature_archive_path() -> &'static str {
    signature_path()
}
