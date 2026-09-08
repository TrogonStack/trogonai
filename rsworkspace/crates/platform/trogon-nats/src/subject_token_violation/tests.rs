use super::*;

#[test]
fn variants_are_equal_to_themselves() {
    assert_eq!(SubjectTokenViolationError::Empty, SubjectTokenViolationError::Empty);
    assert_eq!(
        SubjectTokenViolationError::InvalidCharacter('.'),
        SubjectTokenViolationError::InvalidCharacter('.')
    );
    assert_eq!(SubjectTokenViolationError::TooLong(200), SubjectTokenViolationError::TooLong(200));
}

#[test]
fn variants_are_not_equal_to_each_other() {
    assert_ne!(SubjectTokenViolationError::Empty, SubjectTokenViolationError::TooLong(1));
    assert_ne!(
        SubjectTokenViolationError::InvalidCharacter('*'),
        SubjectTokenViolationError::InvalidCharacter('>')
    );
    assert_ne!(SubjectTokenViolationError::TooLong(10), SubjectTokenViolationError::TooLong(20));
}

#[test]
fn clone_produces_equal_value() {
    let v = SubjectTokenViolationError::InvalidCharacter('x');
    assert_eq!(v.clone(), v);
}

#[test]
fn debug_format_is_non_empty() {
    assert!(!format!("{:?}", SubjectTokenViolationError::Empty).is_empty());
    assert!(!format!("{:?}", SubjectTokenViolationError::InvalidCharacter('.')).is_empty());
    assert!(!format!("{:?}", SubjectTokenViolationError::TooLong(128)).is_empty());
}

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
