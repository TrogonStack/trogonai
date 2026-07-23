use super::*;

const VALID: &str = "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

#[test]
fn accepts_canonical_sha256_digest() {
    assert_eq!(ContentDigest::parse(VALID).unwrap().as_str(), VALID);
}

#[test]
fn rejects_missing_prefix_wrong_length_and_noncanonical_hex() {
    assert_eq!(ContentDigest::parse(""), Err(ContentDigestError::InvalidPrefix));
    assert_eq!(
        ContentDigest::parse("sha256:abc"),
        Err(ContentDigestError::InvalidLength { actual: 3 })
    );
    let uppercase = "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdeF";
    assert!(matches!(
        ContentDigest::parse(uppercase),
        Err(ContentDigestError::InvalidHex { character: 'F', .. })
    ));
}

#[test]
fn supports_standard_string_conversions_and_display() {
    let digest = ContentDigest::parse(VALID).unwrap();

    assert_eq!(digest.as_ref(), VALID);
    assert_eq!(VALID.parse::<ContentDigest>().unwrap(), digest);
    assert_eq!(digest.to_string(), VALID);
}
