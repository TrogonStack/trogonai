use hmac::{Hmac, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

/// Verifies a Linear webhook signature using constant-time comparison.
///
/// Linear sends `linear-signature: <hex>` — a raw lowercase hex-encoded
/// HMAC-SHA256 of the request body, with no prefix.
pub fn verify(secret: &str, body: &[u8], signature_header: &str) -> bool {
    let Ok(expected) = hex::decode(signature_header) else {
        return false;
    };

    let Ok(mut mac) = HmacSha256::new_from_slice(secret.as_bytes()) else {
        return false;
    };

    mac.update(body);
    mac.verify_slice(&expected).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn compute_sig(secret: &str, body: &[u8]) -> String {
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(body);
        hex::encode(mac.finalize().into_bytes())
    }

    #[test]
    fn valid_signature_passes() {
        let sig = compute_sig("test-secret", b"hello world");
        assert!(verify("test-secret", b"hello world", &sig));
    }

    #[test]
    fn wrong_secret_fails() {
        let sig = compute_sig("correct-secret", b"body");
        assert!(!verify("wrong-secret", b"body", &sig));
    }

    #[test]
    fn tampered_body_fails() {
        let sig = compute_sig("secret", b"original body");
        assert!(!verify("secret", b"tampered body", &sig));
    }

    #[test]
    fn invalid_hex_fails() {
        assert!(!verify("secret", b"body", "not-valid-hex!"));
    }

    #[test]
    fn empty_body_with_valid_sig_passes() {
        let sig = compute_sig("secret", b"");
        assert!(verify("secret", b"", &sig));
    }

    #[test]
    fn uppercase_hex_signature_passes() {
        let sig = compute_sig("secret", b"body");
        assert!(verify("secret", b"body", &sig.to_uppercase()));
    }

    #[test]
    fn truncated_signature_fails() {
        let sig = compute_sig("secret", b"body");
        // Remove last two hex chars (one byte) — wrong length, verify_slice must reject
        let truncated = &sig[..sig.len() - 2];
        assert!(!verify("secret", b"body", truncated));
    }
}
