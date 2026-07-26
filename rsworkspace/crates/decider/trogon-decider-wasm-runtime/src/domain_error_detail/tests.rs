use super::*;

#[test]
fn displays_as_code_colon_message() {
    let detail = GuestDomainError {
        code: "already-registered".to_string(),
        message: "schedule already exists".to_string(),
        details: Vec::new(),
    };
    assert_eq!(detail.to_string(), "already-registered: schedule already exists");
}

#[test]
fn implements_error_trait_without_source() {
    let detail = GuestDomainError {
        code: "code".to_string(),
        message: "message".to_string(),
        details: Vec::new(),
    };
    assert!(std::error::Error::source(&detail).is_none());
}

#[test]
fn converts_from_wit_domain_error() {
    let wit_error = trogon_decider_wit::host::DomainError {
        code: "code".to_string(),
        message: "message".to_string(),
        details: vec![("cause".to_string(), "inner failure".to_string())],
    };
    let detail = GuestDomainError::from(wit_error);
    assert_eq!(detail.code, "code");
    assert_eq!(detail.message, "message");
    assert_eq!(detail.details, vec![("cause".to_string(), "inner failure".to_string())]);
}
