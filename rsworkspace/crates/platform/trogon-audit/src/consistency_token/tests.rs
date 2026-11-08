use super::*;

#[test]
fn accepts_opaque_tokens_of_the_shapes_snapshots_use() {
    for value in [
        "GhUKEzE3MDAwMDAwMDAwMDAwMDAwMDA=",
        "CAESAggB+/w=",
        "CAESAggB-_w",
        "rev-12",
        "1700000000.42:7~a",
    ] {
        assert_eq!(ConsistencyToken::new(value).unwrap().as_str(), value);
    }
}

#[test]
fn rejects_empty() {
    assert_eq!(ConsistencyToken::new(""), Err(ConsistencyTokenError::Empty));
}

#[test]
fn rejects_over_length() {
    let long = "a".repeat(MAX_CONSISTENCY_TOKEN_BYTES + 1);
    assert_eq!(
        ConsistencyToken::new(&long),
        Err(ConsistencyTokenError::TooLong(long.len()))
    );
    assert!(ConsistencyToken::new("a".repeat(MAX_CONSISTENCY_TOKEN_BYTES)).is_ok());
}

#[test]
fn rejects_the_characters_content_is_made_of() {
    for (value, character) in [
        ("Authorization: Bearer x", ' '),
        ("{\"a\":1}", '{'),
        ("a\"b", '"'),
        ("a,b", ','),
        ("a;b", ';'),
        ("a?b", '?'),
        ("a\nb", '\n'),
        ("caf\u{e9}", '\u{e9}'),
    ] {
        assert_eq!(
            ConsistencyToken::new(value),
            Err(ConsistencyTokenError::InvalidCharacter(character)),
            "{value:?}"
        );
    }
}

#[test]
fn error_messages_name_the_rule_and_never_the_value() {
    assert_eq!(ConsistencyTokenError::Empty.to_string(), "consistency token is empty");
    assert_eq!(
        ConsistencyTokenError::TooLong(600).to_string(),
        "consistency token is 600 bytes, over the 512-byte limit"
    );
    assert_eq!(
        ConsistencyTokenError::InvalidCharacter(' ').to_string(),
        "consistency token contains a character a token does not use: ' '"
    );
}
