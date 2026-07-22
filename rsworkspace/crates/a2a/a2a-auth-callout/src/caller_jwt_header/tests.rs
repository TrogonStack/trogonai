use super::*;
use crate::jwt::MintedUserJwt;

#[test]
fn from_minted_copies_jwt_string() {
    let token = MintedUserJwt::new("a.b.c").unwrap();
    let header = CallerJwtHeaderValue::from_minted(&token);
    assert_eq!(header.as_str(), "a.b.c");
}

#[test]
fn parse_accepts_trimmed_compact_jwt() {
    let header = CallerJwtHeaderValue::parse("  a.b.c  ").unwrap();
    assert_eq!(header.as_str(), "a.b.c");
}

#[test]
fn parse_rejects_empty_and_whitespace_only() {
    assert!(matches!(
        CallerJwtHeaderValue::parse("").unwrap_err(),
        JwtError::Decode(_)
    ));
    assert!(matches!(
        CallerJwtHeaderValue::parse("   ").unwrap_err(),
        JwtError::Decode(_)
    ));
}

#[test]
fn parse_rejects_non_compact_jwt() {
    for bad in ["a.b", "a..c", "a.b."] {
        assert!(
            matches!(CallerJwtHeaderValue::parse(bad).unwrap_err(), JwtError::Decode(_)),
            "expected rejection for {bad:?}"
        );
    }
}

#[test]
fn display_redacts_value() {
    let header = CallerJwtHeaderValue::parse("a.b.c").unwrap();
    assert_eq!(header.to_string(), "<redacted>");
}
