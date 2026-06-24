use ed25519_dalek::SigningKey;

use super::*;

#[test]
fn from_bytes_and_as_bytes_round_trip() {
    let bytes = [9u8; 32];
    let key = Ed25519PublicKey::from_bytes(bytes);
    assert_eq!(*key.as_bytes(), bytes);
}

#[test]
fn from_hex_parses_valid_key() {
    let signing_key = SigningKey::from_bytes(&[7u8; 32]);
    let hex = hex::encode(signing_key.verifying_key().as_bytes());
    let key = Ed25519PublicKey::from_hex(&hex).expect("valid hex");
    assert_eq!(key.as_bytes(), signing_key.verifying_key().as_bytes());
}

#[test]
fn from_hex_rejects_invalid_hex() {
    let err = Ed25519PublicKey::from_hex("not-hex").expect_err("invalid hex");
    assert!(matches!(err, SignatureVerificationError::MalformedSignatureFile { .. }));
}

#[test]
fn from_hex_rejects_wrong_length() {
    let err = Ed25519PublicKey::from_hex("0102").expect_err("wrong length");
    assert!(matches!(err, SignatureVerificationError::MalformedSignatureFile { .. }));
}

#[test]
fn rejects_0x_prefix() {
    let err = Ed25519PublicKey::from_hex("0x00").expect_err("0x prefix");
    assert!(matches!(err, SignatureVerificationError::MalformedSignatureFile { .. }));
}

#[test]
fn display_encodes_hex() {
    let bytes = [3u8; 32];
    let key = Ed25519PublicKey::from_bytes(bytes);
    assert_eq!(key.to_string(), hex::encode(bytes));
}

#[test]
fn verifying_key_accepts_valid_point() {
    let signing_key = SigningKey::from_bytes(&[7u8; 32]);
    let key = Ed25519PublicKey::from_bytes(*signing_key.verifying_key().as_bytes());
    key.verifying_key().expect("valid verifying key");
}
