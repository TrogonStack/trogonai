use super::{ServiceName, ServiceNameError};
use crate::service_name_input::ServiceNameInput;

#[test]
fn accepts_a_protobuf_service_name() {
    let name = ServiceName::from_input(&ServiceNameInput::new("EchoService")).expect("protobuf service name is valid");
    assert_eq!(name.as_str(), "EchoService");
}

#[test]
fn rejects_empty() {
    assert_eq!(
        ServiceName::from_input(&ServiceNameInput::new("")),
        Err(ServiceNameError::Empty)
    );
}

#[test]
fn rejects_a_leading_digit() {
    assert_eq!(
        ServiceName::from_input(&ServiceNameInput::new("1Echo")),
        Err(ServiceNameError::LeadingCharacter('1'))
    );
}

#[test]
fn rejects_subject_separators_and_wildcards() {
    assert_eq!(
        ServiceName::from_input(&ServiceNameInput::new("echo.v1")),
        Err(ServiceNameError::InvalidCharacter('.'))
    );
    assert_eq!(
        ServiceName::from_input(&ServiceNameInput::new("Echo*")),
        Err(ServiceNameError::InvalidCharacter('*'))
    );
}

#[test]
fn rejects_characters_outside_the_protobuf_identifier_grammar() {
    assert_eq!(
        ServiceName::from_input(&ServiceNameInput::new("Echo-Service")),
        Err(ServiceNameError::InvalidCharacter('-'))
    );
}

#[test]
fn rejects_a_name_over_the_subject_token_budget() {
    let long = "E".repeat(129);
    assert_eq!(
        ServiceName::from_input(&ServiceNameInput::new(long.as_str())),
        Err(ServiceNameError::TooLong(129))
    );
}
