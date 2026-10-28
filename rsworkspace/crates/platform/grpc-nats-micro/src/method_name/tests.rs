use super::{MethodName, MethodNameError};
use crate::method_name_input::MethodNameInput;

#[test]
fn accepts_a_protobuf_method_name() {
    let method = MethodName::from_input(&MethodNameInput::new("Say")).expect("protobuf method name is valid");
    assert_eq!(method.as_str(), "Say");
}

#[test]
fn rejects_empty() {
    assert_eq!(
        MethodName::from_input(&MethodNameInput::new("")),
        Err(MethodNameError::Empty)
    );
}

#[test]
fn rejects_a_leading_digit() {
    assert_eq!(
        MethodName::from_input(&MethodNameInput::new("2Say")),
        Err(MethodNameError::LeadingCharacter('2'))
    );
}

#[test]
fn rejects_subject_separators_and_wildcards() {
    assert_eq!(
        MethodName::from_input(&MethodNameInput::new("Say.Again")),
        Err(MethodNameError::InvalidCharacter('.'))
    );
    assert_eq!(
        MethodName::from_input(&MethodNameInput::new("Say>")),
        Err(MethodNameError::InvalidCharacter('>'))
    );
}

#[test]
fn rejects_characters_outside_the_protobuf_identifier_grammar() {
    assert_eq!(
        MethodName::from_input(&MethodNameInput::new("Say-Again")),
        Err(MethodNameError::InvalidCharacter('-'))
    );
}

#[test]
fn rejects_a_name_over_the_subject_token_budget() {
    let long = "S".repeat(129);
    assert_eq!(
        MethodName::from_input(&MethodNameInput::new(long.as_str())),
        Err(MethodNameError::TooLong(129))
    );
}
