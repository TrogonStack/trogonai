use super::EndpointSubject;
use crate::method_name::MethodName;
use crate::service_name::ServiceName;
use crate::subject_prefix::SubjectPrefix;

fn subject(prefix: &str) -> Result<EndpointSubject, super::EndpointSubjectError> {
    EndpointSubject::new(
        &SubjectPrefix::new(prefix).expect("valid prefix"),
        &ServiceName::new("EchoService").expect("valid service name"),
        &MethodName::new("Say").expect("valid method name"),
    )
}

#[test]
fn derives_prefix_service_method() {
    let derived = subject("echo.v1").expect("derives a conformant subject");
    assert_eq!(derived.as_str(), "echo.v1.EchoService.Say");
}

#[test]
fn rejects_a_subject_over_the_token_budget() {
    let deep = (0..trogon_nats::MAX_SUBJECT_TOKENS)
        .map(|_| "a")
        .collect::<Vec<_>>()
        .join(".");
    let error = subject(&deep).expect_err("a subject over the token budget is rejected");
    assert!(matches!(
        error.source,
        trogon_nats::subject_conformance::SubjectViolationError::TooManyTokens { .. }
    ));
}

#[test]
fn renders_as_the_subject_it_derived() {
    let derived = subject("echo.v1").expect("derives a conformant subject");
    assert_eq!(derived.to_string(), "echo.v1.EchoService.Say");
}

/// The rejected components stay typed, so a caller can act on the one that
/// pushed the derivation over budget.
#[test]
fn reports_the_components_it_derived_from() {
    let deep = (0..trogon_nats::MAX_SUBJECT_TOKENS)
        .map(|_| "a")
        .collect::<Vec<_>>()
        .join(".");
    let error = subject(&deep).expect_err("a subject over the token budget is rejected");

    assert_eq!(error.subject_prefix.as_str(), deep);
    assert_eq!(error.service_name.as_str(), "EchoService");
    assert_eq!(error.method_name.as_str(), "Say");
}
