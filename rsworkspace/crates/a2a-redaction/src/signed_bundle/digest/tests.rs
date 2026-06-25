use super::*;

#[test]
fn hash_is_deterministic() {
    let first = Sha256Digest::hash(b"payload");
    let second = Sha256Digest::hash(b"payload");
    assert_eq!(first, second);
}

#[test]
fn hex_roundtrip() {
    let digest = Sha256Digest::hash(b"x");
    let parsed = Sha256Digest::from_hex(&digest.to_hex()).expect("parse hex");
    assert_eq!(parsed, digest);
}

#[test]
fn from_bytes_and_as_bytes_round_trip() {
    let bytes = [4u8; 32];
    let digest = Sha256Digest::from_bytes(bytes);
    assert_eq!(*digest.as_bytes(), bytes);
}

#[test]
fn from_hex_rejects_wrong_length() {
    let err = Sha256Digest::from_hex("abcd").expect_err("wrong length");
    assert!(matches!(err, hex::FromHexError::InvalidStringLength));
}

#[test]
fn display_encodes_hex() {
    let digest = Sha256Digest::from_bytes([0u8; 32]);
    assert_eq!(digest.to_string(), digest.to_hex());
}
