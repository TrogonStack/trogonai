use super::*;
use crate::fixtures::signing_key;

fn key_id(seed: u8) -> KeyId {
    KeyId::from_verifying_key(signing_key(seed).verifying_key())
}

fn did_key(tagged: &[u8]) -> String {
    format!("did:key:z{}", bs58::encode(tagged).into_string())
}

#[test]
fn an_ed25519_key_id_is_a_did_key_that_parses_back_to_the_key() {
    let key = key_id(1);
    let rendered = key.to_string();

    assert!(rendered.starts_with("did:key:z6Mk"), "{rendered}");
    assert_eq!(KeyId::parse(&rendered), Ok(key));
    assert_eq!(key.verifying_key(), &signing_key(1).verifying_key());
}

#[test]
fn renders_the_did_key_test_vector() {
    // The Ed25519 example in the did:key method specification.
    let public_key: [u8; 32] = [
        0x2e, 0x6f, 0xcc, 0xe3, 0x67, 0x01, 0xdc, 0x79, 0x14, 0x88, 0xe0, 0xd0, 0xb1, 0x74, 0x5c, 0xc1, 0xe3, 0x3a,
        0x4c, 0x1c, 0x9f, 0xcc, 0x41, 0xc6, 0x3b, 0xd3, 0x43, 0xdb, 0xbe, 0x09, 0x70, 0xe6,
    ];
    let key = KeyId::from_verifying_key(VerifyingKey::from_bytes(&public_key).unwrap());
    assert_eq!(
        key.to_string(),
        "did:key:z6MkhaXgBZDvotDkL5257faiztiGiC2QtKLGpbnnEGta2doK"
    );
}

#[test]
fn refuses_what_is_not_a_base58btc_did_key() {
    for value in ["", "did:web:example.com", "did:key:", "did:key:f6Mk", "z6Mk"] {
        assert_eq!(KeyId::parse(value), Err(KeyIdError::NotDidKey), "{value}");
    }
}

#[test]
fn refuses_characters_outside_base58() {
    assert_eq!(KeyId::parse("did:key:z0OIl"), Err(KeyIdError::NotBase58));
}

#[test]
fn refuses_a_key_of_another_type_or_length() {
    let key = *signing_key(1).verifying_key().as_bytes();
    let mut secp256k1 = vec![0xe7, 0x01];
    secp256k1.extend_from_slice(&key);
    let mut short = ED25519_MULTICODEC.to_vec();
    short.extend_from_slice(&key[..31]);
    let mut long = ED25519_MULTICODEC.to_vec();
    long.extend_from_slice(&key);
    long.push(0);

    for tagged in [secp256k1, short, long, Vec::new()] {
        assert_eq!(KeyId::parse(&did_key(&tagged)), Err(KeyIdError::NotEd25519));
    }
}

#[test]
fn refuses_bytes_that_are_not_a_curve_point() {
    let mut tagged = ED25519_MULTICODEC.to_vec();
    tagged.extend_from_slice(&NOT_A_POINT);
    assert_eq!(KeyId::parse(&did_key(&tagged)), Err(KeyIdError::InvalidPublicKey));
}

/// A y coordinate with no x on the curve, so it does not decompress.
const NOT_A_POINT: [u8; 32] = {
    let mut bytes = [0; 32];
    bytes[0] = 2;
    bytes
};
