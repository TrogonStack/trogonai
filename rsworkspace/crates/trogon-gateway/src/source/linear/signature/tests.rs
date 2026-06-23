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

/// An odd-length hex string consists only of valid hex characters but
/// cannot be decoded (each byte needs exactly two nibbles).
/// `hex::decode` returns an error → `verify` must return false.
#[test]
fn odd_length_hex_fails() {
    assert!(!verify("secret", b"body", "abc")); // 3 valid hex chars
    assert!(!verify("secret", b"body", "abcde")); // 5 valid hex chars
    assert!(!verify("secret", b"body", "a")); // 1 valid hex char
}

/// HMAC-SHA256 is well-defined for an empty key.  `verify` must succeed
/// when both sides use the same (empty) secret.
#[test]
fn empty_secret_computes_and_verifies() {
    let sig = compute_sig("", b"hello");
    assert!(verify("", b"hello", &sig));
}

/// A wrong body with an empty secret must still fail.
#[test]
fn empty_secret_wrong_body_fails() {
    let sig = compute_sig("", b"original");
    assert!(!verify("", b"tampered", &sig));
}
