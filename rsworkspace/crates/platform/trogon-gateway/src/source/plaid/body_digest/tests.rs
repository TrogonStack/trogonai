use super::*;

const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
const ABC_SHA256: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

#[test]
fn digests_the_raw_bytes_as_lowercase_hex() {
    assert!(PlaidBodyDigest::of(b"").matches_claim(EMPTY_SHA256));
    assert!(PlaidBodyDigest::of(b"abc").matches_claim(ABC_SHA256));
}

#[test]
fn whitespace_changes_the_digest() {
    assert_ne!(PlaidBodyDigest::of(b"{\"a\":1}"), PlaidBodyDigest::of(b"{\"a\": 1}"));
}

#[test]
fn matches_only_the_exact_claim() {
    let digest = PlaidBodyDigest::of(b"abc");
    assert!(digest.matches_claim(ABC_SHA256));
    assert!(!digest.matches_claim(EMPTY_SHA256));
    assert!(!digest.matches_claim(""));
    assert!(!digest.matches_claim("ba7816bf"));
    assert!(!digest.matches_claim("BA7816BF8F01CFEA414140DE5DAE2223B00361A396177A9CB410FF61F20015AD"));
}
