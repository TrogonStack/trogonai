use sha2::{Digest, Sha256};

use crate::error::StsError;

/// Resolve workload identity (`wkl`) from an actor attestation token.
///
/// v1 accepts SPIFFE URI strings when a trust bundle is configured (dev path) or
/// SHA-256 cert fingerprints indexed in the bundle PEM comments.
pub fn verify_actor_token(actor_token: &str, trust_bundle_pem: &str) -> Result<String, StsError> {
    let trimmed = actor_token.trim();
    if trimmed.is_empty() {
        return Err(StsError::InvalidGrant("actor_token is empty".into()));
    }

    if trimmed.starts_with("spiffe://") {
        if trust_bundle_pem.trim().is_empty() {
            return Err(StsError::InvalidGrant(
                "trust bundle required for SPIFFE actor_token".into(),
            ));
        }
        return Ok(trimmed.to_string());
    }

    if let Some(fingerprint) = trimmed.strip_prefix("sha256:") {
        if trust_bundle_pem.contains(fingerprint) {
            return Ok(format!("spiffe://attested/fingerprint/{fingerprint}"));
        }
        return Err(StsError::InvalidGrant(
            "actor_token fingerprint not in trust bundle".into(),
        ));
    }

    if trimmed.starts_with("eyJ") {
        return Err(StsError::InvalidGrant(
            "SVID-JWT actor verification is not wired in v1; use spiffe:// URI or sha256 fingerprint".into(),
        ));
    }

    Err(StsError::InvalidGrant(format!(
        "unsupported actor_token format: {}",
        &trimmed[..trimmed.len().min(16)]
    )))
}

pub fn cert_fingerprint_sha256(der: &[u8]) -> String {
    hex::encode(Sha256::digest(der))
}
