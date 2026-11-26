use ed25519_dalek::{SIGNATURE_LENGTH, Signer};

use super::*;
use crate::fixtures::signing_key;

fn key_id() -> KeyId {
    KeyId::from_verifying_key(signing_key(1).verifying_key())
}

#[test]
fn the_protected_header_names_the_type_the_algorithm_and_the_key() {
    let key = key_id();
    let json = String::from_utf8(decode_segment(&protected_header(&key)).unwrap()).unwrap();

    assert_eq!(
        json,
        format!(r#"{{"alg":"EdDSA","kid":"{key}","typ":"trogonai-audit-checkpoint+jws"}}"#)
    );
}

#[test]
fn signs_the_header_and_the_statement_and_stores_neither_payload() {
    let header = protected_header(&key_id());
    let input = signing_input(&header, b"statement");
    assert_eq!(input, format!("{header}.c3RhdGVtZW50").into_bytes());

    let signature = signing_key(1).sign(&input);
    let jws = detached(&header, &signature);

    let (stored_header, stored_signature) = split(&jws).unwrap();
    assert_eq!(stored_header, header);
    assert_eq!(decode_segment(stored_signature).unwrap().len(), SIGNATURE_LENGTH);
}

#[test]
fn split_takes_only_three_segments_with_an_empty_payload() {
    assert_eq!(split("a..b"), Some(("a", "b")));
    for jws in ["", "a", "a.b", "a.p.b", "a...b", "a..b.c"] {
        assert_eq!(split(jws), None, "{jws}");
    }
}

#[test]
fn a_segment_is_unpadded_base64url() {
    assert_eq!(decode_segment("c3Q"), Some(b"st".to_vec()));
    assert_eq!(decode_segment("c3Q="), None);
    assert_eq!(decode_segment("c3Q+"), None);
}
