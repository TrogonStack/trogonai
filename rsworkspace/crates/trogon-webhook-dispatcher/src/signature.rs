use hmac::{Hmac, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

/// Signs `body` with `secret` using HMAC-SHA256 and returns `"sha256=<hex>"`.
///
/// Receivers can verify the signature using the same secret and algorithm —
/// compatible with GitHub's `X-Hub-Signature-256` format.
pub fn sign(secret: &str, body: &[u8]) -> String {
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC accepts any key length");
    mac.update(body);
    format!("sha256={}", hex::encode(mac.finalize().into_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn compute(secret: &str, body: &[u8]) -> String {
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(body);
        format!("sha256={}", hex::encode(mac.finalize().into_bytes()))
    }

    #[test]
    fn produces_sha256_prefix() {
        let sig = sign("secret", b"body");
        assert!(sig.starts_with("sha256="));
    }

    #[test]
    fn matches_expected_hmac() {
        assert_eq!(sign("secret", b"hello"), compute("secret", b"hello"));
    }

    #[test]
    fn different_secrets_produce_different_signatures() {
        assert_ne!(sign("a", b"body"), sign("b", b"body"));
    }

    #[test]
    fn different_bodies_produce_different_signatures() {
        assert_ne!(sign("secret", b"a"), sign("secret", b"b"));
    }

    #[test]
    fn empty_body_is_valid() {
        let sig = sign("secret", b"");
        assert!(sig.starts_with("sha256="));
        assert_eq!(sig, compute("secret", b""));
    }
}
