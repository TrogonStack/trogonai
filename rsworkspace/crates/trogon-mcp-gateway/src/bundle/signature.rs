use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use sha2::{Digest, Sha256};

use super::errors::BundleLoadError;
use super::manifest::SIGNATURE_PATH;

const NKEYS_UNAVAILABLE: &str =
    "the `nkeys` crate is not a direct dependency of trogon-mcp-gateway; \
     add `nkeys = {{ workspace = true }}` to enable Ed25519 NKey verification";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustedKeys {
    allowlist: Vec<String>,
}

impl TrustedKeys {
    pub fn from_allowlist<I, S>(keys: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            allowlist: keys.into_iter().map(Into::into).collect(),
        }
    }

    pub fn contains(&self, nkey_pub: &str) -> bool {
        self.allowlist.iter().any(|candidate| candidate == nkey_pub)
    }

    pub fn is_empty(&self) -> bool {
        self.allowlist.is_empty()
    }
}

pub fn manifest_digest_bytes(manifest_bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(manifest_bytes).into()
}

pub fn parse_signature_bytes(raw: &[u8]) -> Result<Vec<u8>, BundleLoadError> {
    if raw.len() == 64 {
        return Ok(raw.to_vec());
    }
    let trimmed = std::str::from_utf8(raw)
        .map_err(|error| BundleLoadError::SignatureMalformed(format!("sig must be UTF-8 or 64 raw bytes: {error}")))?
        .trim();
    if trimmed.len() == 64 && trimmed.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return hex::decode(trimmed)
            .map_err(|error| BundleLoadError::SignatureMalformed(format!("sig hex decode: {error}")));
    }
    STANDARD
        .decode(trimmed.as_bytes())
        .map_err(|error| BundleLoadError::SignatureMalformed(format!("sig base64 decode: {error}")))
}

pub fn verify_manifest_signature(
    manifest_bytes: &[u8],
    signature_bytes: &[u8],
    signer_nkey_pub: &str,
    trusted: &TrustedKeys,
) -> Result<(), BundleLoadError> {
    if trusted.is_empty() {
        return Err(BundleLoadError::SignatureVerificationUnavailable {
            reason: "trusted_signers allowlist is empty",
        });
    }
    if !trusted.contains(signer_nkey_pub) {
        return Err(BundleLoadError::UntrustedSigner {
            nkey_pub: signer_nkey_pub.to_string(),
        });
    }

    let signature = parse_signature_bytes(signature_bytes)?;
    if signature.len() != 64 {
        return Err(BundleLoadError::SignatureMalformed(format!(
            "expected 64-byte Ed25519 signature, got {} bytes",
            signature.len()
        )));
    }

    let _digest = manifest_digest_bytes(manifest_bytes);
    let _signature = signature;

    Err(BundleLoadError::SignatureVerificationUnavailable {
        reason: NKEYS_UNAVAILABLE,
    })
}

pub fn signature_path() -> &'static str {
    SIGNATURE_PATH
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn untrusted_signer_rejected_before_crypto() {
        let trusted = TrustedKeys::from_allowlist(["UABTRUSTED"]);
        let error = verify_manifest_signature(
            b"manifest",
            &[0u8; 64],
            "UABOTHER",
            &trusted,
        )
        .expect_err("must reject");
        assert!(matches!(error, BundleLoadError::UntrustedSigner { .. }));
    }

    #[test]
    fn trusted_signer_reports_dependency_gap() {
        let trusted = TrustedKeys::from_allowlist(["UABTRUSTED"]);
        let error = verify_manifest_signature(
            b"manifest",
            &[0u8; 64],
            "UABTRUSTED",
            &trusted,
        )
        .expect_err("crypto unavailable");
        assert!(matches!(
            error,
            BundleLoadError::SignatureVerificationUnavailable { .. }
        ));
    }

    #[test]
    fn parse_signature_accepts_raw_and_base64() {
        let raw = vec![7u8; 64];
        assert_eq!(parse_signature_bytes(&raw).expect("raw"), raw);
        let encoded = STANDARD.encode(&raw);
        assert_eq!(
            parse_signature_bytes(encoded.as_bytes()).expect("base64"),
            raw
        );
    }
}
