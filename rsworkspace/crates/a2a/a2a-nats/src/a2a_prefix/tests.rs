use super::*;

#[test]
fn a2a_prefix_new_valid() {
    let p = A2aPrefix::new("a2a").unwrap();
    assert_eq!(p.as_str(), "a2a");
    assert_eq!(A2aPrefix::new("my.multi.part").unwrap().as_str(), "my.multi.part");
}

#[test]
fn a2a_prefix_new_invalid_returns_err() {
    assert!(A2aPrefix::new("").is_err());
    assert!(A2aPrefix::new("a2a.*").is_err());
    assert!(A2aPrefix::new("a2a.>").is_err());
    assert!(A2aPrefix::new("a2a prefix").is_err());
    assert!(A2aPrefix::new("a2a\t").is_err());
    assert!(A2aPrefix::new("a2a\n").is_err());
    assert!(A2aPrefix::new("a2a..foo").is_err());
    assert!(A2aPrefix::new(".a2a").is_err());
    assert!(A2aPrefix::new("a2a.").is_err());
    assert!(A2aPrefix::new("a".repeat(129)).is_err());
}

#[test]
fn a2a_prefix_display() {
    let p = A2aPrefix::new("a2a").unwrap();
    assert_eq!(format!("{p}"), "a2a");
}

#[test]
fn a2a_prefix_error_display() {
    assert_eq!(A2aPrefixError::Empty.to_string(), "a2a_prefix must not be empty");
    assert_eq!(
        A2aPrefixError::InvalidCharacter('*').to_string(),
        "a2a_prefix contains invalid character: '*'"
    );
    assert_eq!(
        A2aPrefixError::TooLong(200).to_string(),
        "a2a_prefix is too long: 200 bytes (max 128)"
    );
}
