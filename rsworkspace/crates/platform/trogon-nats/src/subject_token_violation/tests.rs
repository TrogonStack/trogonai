use super::*;

#[test]
fn display_formats_each_variant() {
    assert_eq!(SubjectTokenViolationError::Empty.to_string(), "subject token is empty");
    assert_eq!(
        SubjectTokenViolationError::InvalidCharacter('*').to_string(),
        "subject token contains invalid character '*'"
    );
    assert_eq!(
        SubjectTokenViolationError::TooLong(129).to_string(),
        "subject token exceeds maximum length: 129"
    );
}

#[test]
fn violation_implements_error() {
    let error: &dyn std::error::Error = &SubjectTokenViolationError::Empty;

    assert_eq!(error.to_string(), "subject token is empty");
}
