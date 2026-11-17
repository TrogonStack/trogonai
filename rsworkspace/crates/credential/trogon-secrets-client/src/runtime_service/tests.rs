use trogon_credential_domain::RuntimeServiceId;

use super::RuntimeService;
use crate::RuntimeServiceError;

#[test]
fn refuses_an_id_the_subject_would_carry_as_its_digest() {
    let id = RuntimeServiceId::new("a".repeat(65)).unwrap();

    let error = RuntimeService::new(&id).unwrap_err();

    assert_eq!(error, RuntimeServiceError::NotVerbatim);
    assert_eq!(
        error.to_string(),
        "runtime service id is not carried verbatim by a subject token"
    );
}

#[test]
fn renders_as_the_service_principal_of_a_subject() {
    let id = RuntimeServiceId::new("billing-runner").unwrap();

    let service = RuntimeService::new(&id).unwrap();

    assert_eq!(service.to_string(), "service.billing-runner");
}

#[test]
fn accepts_the_longest_id_a_subject_carries_verbatim() {
    let id = RuntimeServiceId::new("a".repeat(64)).unwrap();

    let service = RuntimeService::new(&id).unwrap();

    assert_eq!(service.to_string(), format!("service.{}", "a".repeat(64)));
}
