use super::ServiceVersion;
use crate::service_version_input::ServiceVersionInput;

#[test]
fn accepts_a_semantic_version() {
    let version = ServiceVersion::from_input(&ServiceVersionInput::new("1.0.0")).expect("a semantic version");
    assert_eq!(version.as_str(), "1.0.0");
}

#[test]
fn accepts_a_prerelease_and_build_version() {
    let version =
        ServiceVersion::from_input(&ServiceVersionInput::new("1.0.0-rc.1+build.7")).expect("a semantic version");
    assert_eq!(version.to_string(), "1.0.0-rc.1+build.7");
}

/// NATS micro rejects a bare major at startup, so the binding rejects it first.
#[test]
fn rejects_a_version_that_is_not_semantic() {
    let error =
        ServiceVersion::from_input(&ServiceVersionInput::new("1")).expect_err("a bare major is not a semantic version");
    assert_eq!(error.to_string(), "service version is not a semantic version");
}
