use crate::ard_identifier::ArdIdentifier;

use super::*;

#[test]
fn derives_stable_lowercase_hex_sha256() {
    let identifier = ArdIdentifier::new("urn:air:example.com:agent:assistant").unwrap();
    let key = ArdStorageKey::from_identifier(&identifier);
    assert_eq!(key.as_str().len(), 64);
    assert!(
        key.as_str()
            .chars()
            .all(|ch| ch.is_ascii_hexdigit() && !ch.is_uppercase())
    );

    let again = ArdStorageKey::from_identifier(&identifier);
    assert_eq!(key, again);
}

#[test]
fn is_nats_token_safe() {
    let identifier = ArdIdentifier::new("urn:air:example.com:agent:assistant").unwrap();
    let key = ArdStorageKey::from_identifier(&identifier);
    assert!(!key.as_str().is_empty());
    assert!(key.as_str().is_ascii());
    assert!(
        key.as_str()
            .chars()
            .all(|ch| ch != '.' && ch != '*' && ch != '>' && !ch.is_whitespace())
    );
}
