use std::fmt;

use hmac::{Hmac, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug)]
#[non_exhaustive]
pub enum SignatureError {
    MissingPrefix,
    InvalidHex(hex::FromHexError),
    Mismatch,
}

impl fmt::Display for SignatureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SignatureError::MissingPrefix => f.write_str("missing sha256= prefix"),
            SignatureError::InvalidHex(_) => f.write_str("invalid hex encoding"),
            SignatureError::Mismatch => f.write_str("signature mismatch"),
        }
    }
}

impl std::error::Error for SignatureError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            SignatureError::InvalidHex(e) => Some(e),
            _ => None,
        }
    }
}

/// Verifies a GitHub webhook signature using constant-time comparison.
///
/// GitHub sends `X-Hub-Signature-256: sha256=<hex>`. This function validates
/// the HMAC-SHA256 of the raw request body against that header value.
pub fn verify(secret: &str, body: &[u8], signature_header: &str) -> Result<(), SignatureError> {
    let hex_sig = signature_header
        .strip_prefix("sha256=")
        .ok_or(SignatureError::MissingPrefix)?;

    let expected = hex::decode(hex_sig).map_err(SignatureError::InvalidHex)?;

    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC-SHA256 accepts any key length");

    mac.update(body);
    mac.verify_slice(&expected).map_err(|_| SignatureError::Mismatch)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error;

    fn compute_sig(secret: &str, body: &[u8]) -> String {
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(body);
        format!("sha256={}", hex::encode(mac.finalize().into_bytes()))
    }

    #[test]
    fn error_display_messages() {
        assert_eq!(SignatureError::MissingPrefix.to_string(), "missing sha256= prefix");
        let hex_err = SignatureError::InvalidHex(hex::decode("zz").unwrap_err());
        assert_eq!(hex_err.to_string(), "invalid hex encoding");
        assert!(hex_err.source().is_some());
        assert_eq!(SignatureError::Mismatch.to_string(), "signature mismatch");
        assert!(SignatureError::Mismatch.source().is_none());
    }

    #[test]
    fn valid_signature_passes() {
        let sig = compute_sig("test-secret", b"hello world");
        assert!(verify("test-secret", b"hello world", &sig).is_ok());
    }

    #[test]
    fn wrong_secret_fails() {
        let sig = compute_sig("correct-secret", b"body");
        assert!(matches!(
            verify("wrong-secret", b"body", &sig),
            Err(SignatureError::Mismatch)
        ));
    }

    #[test]
    fn tampered_body_fails() {
        let sig = compute_sig("secret", b"original body");
        assert!(matches!(
            verify("secret", b"tampered body", &sig),
            Err(SignatureError::Mismatch)
        ));
    }

    #[test]
    fn missing_sha256_prefix_fails() {
        let sig = compute_sig("secret", b"body");
        let raw_hex = sig.strip_prefix("sha256=").unwrap().to_string();
        assert!(matches!(
            verify("secret", b"body", &raw_hex),
            Err(SignatureError::MissingPrefix)
        ));
    }

    #[test]
    fn invalid_hex_fails() {
        assert!(matches!(
            verify("secret", b"body", "sha256=not-valid-hex!"),
            Err(SignatureError::InvalidHex(_))
        ));
    }

    #[test]
    fn empty_body_with_valid_sig_passes() {
        let sig = compute_sig("secret", b"");
        assert!(verify("secret", b"", &sig).is_ok());
    }
}
