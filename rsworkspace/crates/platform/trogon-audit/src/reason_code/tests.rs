use super::*;

#[test]
fn accepts_lower_snake_codes() {
    let code = ReasonCode::new("policy_denied_2").unwrap();
    assert_eq!(code.as_str(), "policy_denied_2");
    assert_eq!(code.to_string(), "policy_denied_2");
}

#[test]
fn rejects_empty() {
    assert_eq!(ReasonCode::new(""), Err(ReasonCodeError::Empty));
}

#[test]
fn rejects_over_length() {
    let long = "a".repeat(65);
    assert_eq!(
        ReasonCode::new(Box::leak(long.into_boxed_str())),
        Err(ReasonCodeError::TooLong(65))
    );
}

#[test]
fn rejects_anything_but_lower_snake_starting_with_a_letter() {
    for code in ["Denied", "denied code", "_denied", "1denied", "denied-code", "denied."] {
        assert_eq!(ReasonCode::new(code), Err(ReasonCodeError::NotLowerSnake), "{code}");
    }
}

#[test]
fn error_messages_state_the_rule() {
    for error in [
        ReasonCodeError::Empty,
        ReasonCodeError::TooLong(65),
        ReasonCodeError::NotLowerSnake,
    ] {
        assert!(!error.to_string().is_empty());
    }
}
