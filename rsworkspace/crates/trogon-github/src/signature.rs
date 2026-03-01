use hmac::{Hmac, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

/// Verifies a GitHub webhook signature using constant-time comparison.
///
/// GitHub sends `X-Hub-Signature-256: sha256=<hex>`. This function validates
/// the HMAC-SHA256 of the raw request body against that header value.
pub fn verify(secret: &str, body: &[u8], signature_header: &str) -> bool {
    let hex_sig = match signature_header.strip_prefix("sha256=") {
        Some(s) => s,
        None => return false,
    };

    let Ok(expected) = hex::decode(hex_sig) else {
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
        format!("sha256={}", hex::encode(mac.finalize().into_bytes()))
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
    fn missing_sha256_prefix_fails() {
        let sig = compute_sig("secret", b"body");
        let raw_hex = sig.strip_prefix("sha256=").unwrap().to_string();
        assert!(!verify("secret", b"body", &raw_hex));
    }

    #[test]
    fn invalid_hex_fails() {
        assert!(!verify("secret", b"body", "sha256=not-valid-hex!"));
    }

    #[test]
    fn empty_body_with_valid_sig_passes() {
        let sig = compute_sig("secret", b"");
        assert!(verify("secret", b"", &sig));
    }
}
