use super::{SubjectPrefix, SubjectPrefixError};

#[test]
fn accepts_a_dotted_namespace() {
    let prefix = SubjectPrefix::new("echo.v1").expect("dotted prefix is valid");
    assert_eq!(prefix.as_str(), "echo.v1");
}

#[test]
fn rejects_empty() {
    assert_eq!(SubjectPrefix::new(""), Err(SubjectPrefixError::Empty));
}

#[test]
fn rejects_wildcards() {
    assert_eq!(
        SubjectPrefix::new("echo.*"),
        Err(SubjectPrefixError::InvalidCharacter('*'))
    );
    assert_eq!(
        SubjectPrefix::new("echo.>"),
        Err(SubjectPrefixError::InvalidCharacter('>'))
    );
}

#[test]
fn rejects_malformed_dots() {
    assert_eq!(
        SubjectPrefix::new(".echo"),
        Err(SubjectPrefixError::InvalidCharacter('.'))
    );
    assert_eq!(
        SubjectPrefix::new("echo."),
        Err(SubjectPrefixError::InvalidCharacter('.'))
    );
    assert_eq!(
        SubjectPrefix::new("echo..v1"),
        Err(SubjectPrefixError::InvalidCharacter('.'))
    );
}

#[test]
fn rejects_whitespace() {
    assert_eq!(
        SubjectPrefix::new("echo v1"),
        Err(SubjectPrefixError::InvalidCharacter(' '))
    );
}

#[test]
fn rejects_a_prefix_over_the_subject_token_budget() {
    let long = "e".repeat(129);
    assert_eq!(SubjectPrefix::new(&long), Err(SubjectPrefixError::TooLong(129)));
}
