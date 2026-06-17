use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum SignatureError {
    #[error("missing v0= prefix")]
    MissingPrefix,
    #[error("invalid hex encoding")]
    InvalidHex(#[source] hex::FromHexError),
    #[error("invalid HMAC key")]
    InvalidKey(#[source] hmac::digest::InvalidLength),
    #[error("signature mismatch")]
    Mismatch,
}

/// Verifies a Slack request signature using constant-time comparison.
///
/// Slack sends `X-Slack-Signature: v0=<hex>` and `X-Slack-Request-Timestamp`.
/// The HMAC-SHA256 is computed over `v0:{timestamp}:{body}`.
pub fn verify(
    signing_secret: &str,
    timestamp: &str,
    body: &[u8],
    signature_header: &str,
) -> Result<(), SignatureError> {
    let hex_sig = signature_header
        .strip_prefix("v0=")
        .ok_or(SignatureError::MissingPrefix)?;

    let expected = hex::decode(hex_sig).map_err(SignatureError::InvalidHex)?;

    let mut mac = HmacSha256::new_from_slice(signing_secret.as_bytes()).map_err(SignatureError::InvalidKey)?;

    mac.update(b"v0:");
    mac.update(timestamp.as_bytes());
    mac.update(b":");
    mac.update(body);

    mac.verify_slice(&expected).map_err(|_| SignatureError::Mismatch)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error;

    fn compute_sig(secret: &str, timestamp: &str, body: &[u8]) -> String {
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(b"v0:");
        mac.update(timestamp.as_bytes());
        mac.update(b":");
        mac.update(body);
        format!("v0={}", hex::encode(mac.finalize().into_bytes()))
    }

    #[test]
    fn error_display_messages() {
        assert_eq!(SignatureError::MissingPrefix.to_string(), "missing v0= prefix");
        let hex_err = SignatureError::InvalidHex(hex::decode("zz").unwrap_err());
        assert_eq!(hex_err.to_string(), "invalid hex encoding");
        assert!(hex_err.source().is_some());
        assert_eq!(SignatureError::Mismatch.to_string(), "signature mismatch");
        assert!(SignatureError::Mismatch.source().is_none());
    }

    #[test]
    fn valid_signature_passes() {
        let sig = compute_sig("test-secret", "1234567890", b"hello world");
        assert!(verify("test-secret", "1234567890", b"hello world", &sig).is_ok());
    }

    #[test]
    fn wrong_secret_fails() {
        let sig = compute_sig("correct-secret", "1234567890", b"body");
        assert!(matches!(
            verify("wrong-secret", "1234567890", b"body", &sig),
            Err(SignatureError::Mismatch)
        ));
    }

    #[test]
    fn tampered_body_fails() {
        let sig = compute_sig("secret", "1234567890", b"original body");
        assert!(matches!(
            verify("secret", "1234567890", b"tampered body", &sig),
            Err(SignatureError::Mismatch)
        ));
    }

    #[test]
    fn wrong_timestamp_fails() {
        let sig = compute_sig("secret", "1234567890", b"body");
        assert!(matches!(
            verify("secret", "9999999999", b"body", &sig),
            Err(SignatureError::Mismatch)
        ));
    }

    #[test]
    fn missing_v0_prefix_fails() {
        let sig = compute_sig("secret", "1234567890", b"body");
        let raw_hex = sig.strip_prefix("v0=").unwrap().to_string();
        assert!(matches!(
            verify("secret", "1234567890", b"body", &raw_hex),
            Err(SignatureError::MissingPrefix)
        ));
    }

    #[test]
    fn invalid_hex_fails() {
        assert!(matches!(
            verify("secret", "1234567890", b"body", "v0=not-valid-hex!"),
            Err(SignatureError::InvalidHex(_))
        ));
    }

    #[test]
    fn empty_body_with_valid_sig_passes() {
        let sig = compute_sig("secret", "1234567890", b"");
        assert!(verify("secret", "1234567890", b"", &sig).is_ok());
    }
}
