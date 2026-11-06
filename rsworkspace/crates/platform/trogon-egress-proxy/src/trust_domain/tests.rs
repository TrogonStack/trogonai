use super::*;

#[test]
fn accepts_lowercase_dns_style_names() {
    for raw in ["trogon.example", "a", "a-b.c_d", "dev1.local"] {
        assert_eq!(TrustDomain::new(raw).expect("valid").as_str(), raw);
    }
}

#[test]
fn rejects_empty_and_over_long() {
    assert_eq!(TrustDomain::new(""), Err(TrustDomainError::Empty));
    assert_eq!(
        TrustDomain::new("a".repeat(256)),
        Err(TrustDomainError::TooLong { max: 255 })
    );
}

#[test]
fn rejects_uppercase_and_url_syntax() {
    for raw in ["Trogon.example", "a/b", "a:443", "a b", "a@b", "a?x", "a#x", "é"] {
        assert!(
            matches!(TrustDomain::new(raw), Err(TrustDomainError::InvalidCharacter { .. })),
            "{raw:?} must be rejected"
        );
    }
}
