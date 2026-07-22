//! incident.io webhook signature verification.
//!
//! incident.io sends `X-Incident-Signature: <hex>` — a raw hex-encoded
//! HMAC-SHA256 of the raw request body (no prefix).

use hmac::{Hmac, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

/// Verify an incident.io webhook signature using constant-time comparison.
///
/// Returns `true` when `signature_header` is the correct HMAC-SHA256 hex
/// digest of `body` under `secret`.
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
        let sig = compute_sig("my-secret", b"hello");
        assert!(verify("my-secret", b"hello", &sig));
    }

    #[test]
    fn wrong_secret_fails() {
        let sig = compute_sig("correct", b"body");
        assert!(!verify("wrong", b"body", &sig));
    }

    #[test]
    fn tampered_body_fails() {
        let sig = compute_sig("secret", b"original");
        assert!(!verify("secret", b"tampered", &sig));
    }

    #[test]
    fn invalid_hex_fails() {
        assert!(!verify("secret", b"body", "not-hex!!"));
    }

    #[test]
    fn empty_body_valid_sig_passes() {
        let sig = compute_sig("secret", b"");
        assert!(verify("secret", b"", &sig));
    }

    #[test]
    fn prefix_style_signature_fails() {
        // incident.io does NOT use a "sha256=" prefix — reject if present.
        let sig = compute_sig("secret", b"body");
        let with_prefix = format!("sha256={sig}");
        assert!(!verify("secret", b"body", &with_prefix));
    }

    #[test]
    fn uppercase_hex_signature_is_accepted() {
        // hex::decode is case-insensitive — document that incident.io senders
        // using uppercase hex are still accepted.
        let sig = compute_sig("secret", b"payload").to_uppercase();
        assert!(verify("secret", b"payload", &sig));
    }

    #[test]
    fn mixed_case_hex_signature_is_accepted() {
        let sig = compute_sig("secret", b"payload");
        // Alternate case of every character.
        let mixed: String = sig
            .chars()
            .enumerate()
            .map(|(i, c)| if i % 2 == 0 { c.to_ascii_uppercase() } else { c })
            .collect();
        assert!(verify("secret", b"payload", &mixed));
    }
}
