use super::*;

#[test]
fn genesis_is_64_zero_chars() {
    let genesis = ChainHash::genesis();
    assert_eq!(genesis.as_str(), "0".repeat(HEX_LEN));
}

#[test]
fn digest_is_deterministic() {
    let a = ChainHash::digest(b"hello");
    let b = ChainHash::digest(b"hello");
    assert_eq!(a, b);
}

#[test]
fn digest_differs_for_different_input() {
    let a = ChainHash::digest(b"hello");
    let b = ChainHash::digest(b"world");
    assert_ne!(a, b);
}

#[test]
fn digest_is_lowercase_hex_of_expected_length() {
    let hash = ChainHash::digest(b"hello");
    assert_eq!(hash.as_str().len(), HEX_LEN);
    assert!(
        hash.as_str()
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    );
}

#[test]
fn digest_matches_known_sha256_vector() {
    // NIST test vector: SHA-256("abc")
    let hash = ChainHash::digest(b"abc");
    assert_eq!(
        hash.as_str(),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}

#[test]
fn parse_accepts_valid_hash() {
    let hash = ChainHash::digest(b"abc");
    let parsed = ChainHash::parse(hash.as_str()).expect("valid hash should parse");
    assert_eq!(parsed, hash);
}

#[test]
fn parse_rejects_wrong_length() {
    let err = ChainHash::parse("abc").expect_err("short input should fail");
    assert_eq!(err, ChainHashParseError::WrongLength { found: 3 });
}

#[test]
fn parse_rejects_uppercase_hex() {
    let upper = "A".repeat(HEX_LEN);
    let err = ChainHash::parse(&upper).expect_err("uppercase input should fail");
    assert_eq!(err, ChainHashParseError::NotLowercaseHex);
}

#[test]
fn parse_rejects_non_hex_characters() {
    let mut value = "0".repeat(HEX_LEN - 1);
    value.push('z');
    let err = ChainHash::parse(&value).expect_err("non-hex input should fail");
    assert_eq!(err, ChainHashParseError::NotLowercaseHex);
}

#[test]
fn display_matches_as_str() {
    let hash = ChainHash::genesis();
    assert_eq!(hash.to_string(), hash.as_str());
}
