use super::*;

#[test]
fn a_lowercase_sha256_digest_parses() {
    let digest = "0123456789abcdef".repeat(4);
    assert_eq!(PeerCertificate::try_from(digest.clone()), Ok(PeerCertificate(digest)));
}

#[test]
fn anything_but_a_lowercase_sha256_digest_is_refused() {
    for value in [
        "",
        "abc",
        &"A".repeat(64),
        &"g".repeat(64),
        &"a".repeat(63),
        &"a".repeat(65),
    ] {
        assert_eq!(
            PeerCertificate::try_from(value.to_owned()),
            Err(PeerCertificateError::NotSha256Hex),
            "{value}"
        );
    }
}
