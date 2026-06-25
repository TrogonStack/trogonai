use super::*;

#[test]
fn from_bytes_and_as_bytes_round_trip() {
    let bytes = [5u8; 64];
    let sig = Ed25519Signature::from_bytes(bytes);
    assert_eq!(*sig.as_bytes(), bytes);
}

#[test]
fn from_hex_parses_valid_signature() {
    let bytes = [1u8; 64];
    let sig = Ed25519Signature::from_hex(&hex::encode(bytes)).expect("valid hex");
    assert_eq!(*sig.as_bytes(), bytes);
}

#[test]
fn from_hex_rejects_invalid_hex() {
    let err = Ed25519Signature::from_hex("not-hex").expect_err("invalid hex");
    assert!(matches!(err, SignatureVerificationError::MalformedSignatureFile { .. }));
}

#[test]
fn from_hex_rejects_wrong_length() {
    let err = Ed25519Signature::from_hex("0102").expect_err("wrong length");
    assert!(matches!(err, SignatureVerificationError::MalformedSignatureFile { .. }));
}

#[test]
fn display_encodes_hex() {
    let bytes = [2u8; 64];
    let sig = Ed25519Signature::from_bytes(bytes);
    assert_eq!(sig.to_string(), hex::encode(bytes));
}
