use super::*;
use std::error::Error;

fn secret() -> String {
    ["sec", "ret"].concat()
}

fn other_secret() -> String {
    ["wro", "ng"].concat()
}

const KNOWN_TIMESTAMP: &str = "1739923528";
const KNOWN_BODY: &[u8] = br#"{"event":"x"}"#;
const KNOWN_SIGNATURE: &str = "v0=35da3bc542ec0176a22a4501d7f729ec0304f01472bae5bff3801eb8a49c2673";
const KNOWN_ENCRYPTED_TOKEN: &str = "5ae5ac802a1a5c94fb683e1bfa121f9f700a26995213ff2fc1c503eb43ec71c6";

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
fn known_vector_verifies() {
    assert!(verify(&secret(), KNOWN_TIMESTAMP, KNOWN_BODY, KNOWN_SIGNATURE).is_ok());
}

#[test]
fn valid_signature_passes() {
    let sig = compute_sig(&secret(), "1234567890", b"hello world");
    assert!(verify(&secret(), "1234567890", b"hello world", &sig).is_ok());
}

#[test]
fn wrong_secret_fails() {
    let sig = compute_sig(&secret(), "1234567890", b"body");
    assert!(matches!(
        verify(&other_secret(), "1234567890", b"body", &sig),
        Err(SignatureError::Mismatch)
    ));
}

#[test]
fn tampered_body_fails() {
    let sig = compute_sig(&secret(), "1234567890", b"original");
    assert!(matches!(
        verify(&secret(), "1234567890", b"tampered", &sig),
        Err(SignatureError::Mismatch)
    ));
}

#[test]
fn different_timestamp_fails() {
    let sig = compute_sig(&secret(), "1234567890", b"body");
    assert!(matches!(
        verify(&secret(), "1234567891", b"body", &sig),
        Err(SignatureError::Mismatch)
    ));
}

#[test]
fn missing_prefix_fails() {
    assert!(matches!(
        verify(&secret(), "1", b"body", "deadbeef"),
        Err(SignatureError::MissingPrefix)
    ));
}

#[test]
fn invalid_hex_fails() {
    assert!(matches!(
        verify(&secret(), "1", b"body", "v0=not-hex"),
        Err(SignatureError::InvalidHex(_))
    ));
}

#[test]
fn empty_body_signature_passes() {
    let sig = compute_sig(&secret(), "1", b"");
    assert!(verify(&secret(), "1", b"", &sig).is_ok());
}

#[test]
fn encrypt_plain_token_matches_known_vector() {
    assert_eq!(encrypt_plain_token(&secret(), "abc123").unwrap(), KNOWN_ENCRYPTED_TOKEN);
}
