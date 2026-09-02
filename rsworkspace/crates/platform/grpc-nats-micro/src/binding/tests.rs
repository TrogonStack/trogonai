use super::ServiceBinding;
use crate::discovery_metadata::DiscoveryMetadata;
use crate::method_name::MethodName;
use crate::method_name_input::MethodNameInput;
use crate::service_name::ServiceName;
use crate::service_name_input::ServiceNameInput;
use crate::service_version::ServiceVersion;
use crate::service_version_input::ServiceVersionInput;
use crate::subject_prefix::SubjectPrefix;
use crate::subject_prefix_input::SubjectPrefixInput;

const SUBJECT_PREFIX: &str = "echo.v1";

fn binding() -> ServiceBinding {
    ServiceBinding::new(
        ServiceName::from_input(&ServiceNameInput::new("EchoService")).expect("valid service name"),
        ServiceVersion::from_input(&ServiceVersionInput::new("1.0.0")).expect("valid service version"),
        SubjectPrefix::from_input(&SubjectPrefixInput::new(SUBJECT_PREFIX)).expect("valid subject prefix"),
    )
    .with_method(
        MethodName::from_input(&MethodNameInput::new("Say")).expect("valid method name"),
        DiscoveryMetadata::default(),
    )
    .expect("derive the Say subject")
}

#[test]
fn derives_endpoint_subjects_under_its_own_prefix() {
    let binding = binding();

    assert_eq!(binding.subject_prefix().as_str(), SUBJECT_PREFIX);
    let endpoint = binding.endpoints().first().expect("the Say endpoint is registered");
    assert_eq!(endpoint.method_name().as_str(), "Say");
    assert_eq!(endpoint.subject().as_str(), "echo.v1.EchoService.Say");
}

#[test]
fn carries_the_description_micro_discovery_reports() {
    assert_eq!(binding().description(), None);
    assert_eq!(
        binding().with_description("Echoes what it is told").description(),
        Some("Echoes what it is told")
    );
}

/// The derivation is what can fail, so registering a method has to surface
/// that failure rather than register an endpoint nobody can reach.
#[test]
fn rejects_a_method_whose_subject_is_not_derivable() {
    let deep = (0..trogon_nats::MAX_SUBJECT_TOKENS)
        .map(|_| "a")
        .collect::<Vec<_>>()
        .join(".");
    let error = ServiceBinding::new(
        ServiceName::from_input(&ServiceNameInput::new("EchoService")).expect("valid service name"),
        ServiceVersion::from_input(&ServiceVersionInput::new("1.0.0")).expect("valid service version"),
        SubjectPrefix::from_input(&SubjectPrefixInput::new(deep.as_str())).expect("valid subject prefix"),
    )
    .with_method(
        MethodName::from_input(&MethodNameInput::new("Say")).expect("valid method name"),
        DiscoveryMetadata::default(),
    )
    .expect_err("a subject over the token budget is rejected");

    assert_eq!(error.method_name.as_str(), "Say");
}
