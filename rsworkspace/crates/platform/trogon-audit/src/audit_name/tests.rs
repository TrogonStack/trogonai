use super::*;

#[test]
fn accepts_names_of_the_shapes_a_record_uses() {
    for value in [
        "api.example.com",
        "policies/egress",
        "operator@host-1",
        "https://issuer.example/realm",
        "agent-7",
        "tools:read",
        "[::1]",
        "a+b",
        "user~1",
    ] {
        assert_eq!(AuditName::new(value).unwrap().as_str(), value);
    }
}

#[test]
fn displays_the_name() {
    assert_eq!(AuditName::new("host").unwrap().to_string(), "host");
}

#[test]
fn rejects_empty() {
    assert_eq!(AuditName::new(""), Err(AuditNameError::Empty));
}

#[test]
fn rejects_over_length() {
    let long = "a".repeat(crate::constants::MAX_NAME_BYTES + 1);
    assert_eq!(AuditName::new(&long), Err(AuditNameError::TooLong(long.len())));
    assert!(AuditName::new("a".repeat(crate::constants::MAX_NAME_BYTES)).is_ok());
}

#[test]
fn rejects_the_characters_content_is_made_of() {
    for (value, character) in [
        ("Authorization: Bearer x", ' '),
        ("token=abc", '='),
        ("{\"a\":1}", '{'),
        ("a,b", ','),
        ("a;b", ';'),
        ("a\nb", '\n'),
        ("a?b", '?'),
        ("caf\u{e9}", '\u{e9}'),
        ("a\"b", '"'),
    ] {
        assert_eq!(
            AuditName::new(value),
            Err(AuditNameError::InvalidCharacter(character)),
            "{value:?}"
        );
    }
}

#[test]
fn error_messages_name_the_rule_and_never_the_value() {
    assert_eq!(AuditNameError::Empty.to_string(), "audit name is empty");
    assert_eq!(
        AuditNameError::TooLong(300).to_string(),
        "audit name is 300 bytes, over the 256-byte limit"
    );
    let error = AuditName::new("secret value").unwrap_err();
    assert_eq!(
        error.to_string(),
        "audit name contains a character a name does not use: ' '"
    );
}
