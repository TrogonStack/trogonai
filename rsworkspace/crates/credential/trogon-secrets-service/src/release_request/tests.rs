use std::num::NonZeroU16;

use nkeys::KeyPair;
use trogon_credential_domain::{CredentialId, CredentialOwnerId, ReleaseHost};

use super::*;

fn request() -> ReleaseRequest {
    ReleaseRequest::new(
        TenantAccount::parse(&KeyPair::new_account().public_key()).unwrap(),
        RuntimeServiceId::new("billing-runner").unwrap(),
        CredentialKey::new(
            CredentialOwnerId::new("project-1").unwrap(),
            CredentialId::new("0190a5b4-7c1e-7d2a-9f3b-6c8d4e2a1b0f").unwrap(),
        ),
        ReleaseDestination::new(
            ReleaseHost::new("api.github.com").unwrap(),
            NonZeroU16::new(443).unwrap(),
        ),
        InjectionLocation::header("authorization").unwrap(),
        AuditCorrelationId::new("use-1").unwrap(),
    )
}

#[test]
fn keeps_every_part_of_the_request() {
    let request = request();

    assert_eq!(request.runtime_service().as_str(), "billing-runner");
    assert_eq!(request.credential().owner_id().as_str(), "project-1");
    assert_eq!(request.destination().port().get(), 443);
    assert_eq!(request.location(), &InjectionLocation::header("authorization").unwrap());
    assert_eq!(request.correlation_id().as_str(), "use-1");
    assert!(request.tenant().as_str().starts_with('A'));
}

#[test]
fn hands_the_policy_the_runtime_service_host_and_location_only() {
    let request = request();

    let policy_request = request.policy_request();

    assert_eq!(policy_request.runtime_service(), request.runtime_service());
    assert_eq!(policy_request.host(), request.destination().host());
    assert_eq!(policy_request.location(), request.location());
}
