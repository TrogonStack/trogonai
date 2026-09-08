use super::*;

#[test]
fn rejects_empty_token() {
    assert_eq!(DottedNatsToken::new(""), Err(SubjectTokenViolationError::Empty));
}

#[test]
fn rejects_token_over_max_length() {
    let value = "a".repeat(129);
    assert_eq!(
        DottedNatsToken::new(&value),
        Err(SubjectTokenViolationError::TooLong(129))
    );
}

#[test]
fn accepts_and_displays_valid_token() {
    let token = DottedNatsToken::new("agent.run").unwrap();
    assert_eq!(token.as_str(), "agent.run");
    assert_eq!(token.to_string(), "agent.run");
}
