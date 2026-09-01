use super::ServiceBinding;
use crate::method_name::MethodName;
use crate::service_name::ServiceName;
use crate::service_version::ServiceVersion;
use crate::subject_prefix::SubjectPrefix;

const SUBJECT_PREFIX: &str = "echo.v1";

fn binding() -> ServiceBinding {
    ServiceBinding::new(
        ServiceName::new("EchoService").expect("valid service name"),
        ServiceVersion::new("1.0.0").expect("valid service version"),
        SubjectPrefix::new(SUBJECT_PREFIX).expect("valid subject prefix"),
    )
    .with_method(MethodName::new("Say").expect("valid method name"))
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
