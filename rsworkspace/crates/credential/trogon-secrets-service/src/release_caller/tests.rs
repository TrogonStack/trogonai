use nkeys::KeyPair;
use trogon_identity_types::{PrincipalId, PrincipalKind, PrincipalKindError, SubjectPrincipal, SubjectPrincipalError};

use super::*;
use crate::TenantAccountError;
use crate::fixtures::{RUNTIME_SERVICE, tenant};

fn subject(tenant: &str, kind: &str, id: &str) -> String {
    format!("{tenant}.secrets.v1.{kind}.{id}.SecretExchangeService.ResolveForRelease")
}

#[test]
fn a_subject_outside_the_release_endpoint_names_no_caller() {
    let tenant = tenant();
    let tenant = tenant.as_str();
    for outside in [
        format!("secrets.v1.service.{RUNTIME_SERVICE}.SecretExchangeService.ResolveForRelease"),
        format!("{tenant}.secrets.v2.service.{RUNTIME_SERVICE}.SecretExchangeService.ResolveForRelease"),
        format!("{tenant}.vault.v1.service.{RUNTIME_SERVICE}.SecretExchangeService.ResolveForRelease"),
        format!("{tenant}.secrets.v1.service.{RUNTIME_SERVICE}.SecretExchangeService.ResolveForRefresh"),
        format!("{tenant}.secrets.v1.service.{RUNTIME_SERVICE}.OtherService.ResolveForRelease"),
        format!("{tenant}.secrets.v1.service.{RUNTIME_SERVICE}.SecretExchangeService.ResolveForRelease.extra"),
        format!("{tenant}.secrets.v1.service.SecretExchangeService.ResolveForRelease"),
        String::new(),
    ] {
        assert_eq!(
            ReleaseCaller::from_subject(&outside),
            Err(ReleaseCallerError::OutsideEndpoint),
            "{outside}"
        );
    }
}

#[test]
fn a_tenant_token_that_is_not_an_account_key_names_no_caller() {
    assert_eq!(
        ReleaseCaller::from_subject(&subject("fake-tenant", "service", RUNTIME_SERVICE)),
        Err(ReleaseCallerError::Tenant(TenantAccountError::NotAPublicKey))
    );
    assert_eq!(
        ReleaseCaller::from_subject(&subject(&KeyPair::new_user().public_key(), "service", RUNTIME_SERVICE)),
        Err(ReleaseCallerError::Tenant(TenantAccountError::NotAnAccount))
    );
}

#[test]
fn a_principal_of_another_kind_names_no_caller() {
    let tenant = tenant();
    assert_eq!(
        ReleaseCaller::from_subject(&subject(tenant.as_str(), "agent", RUNTIME_SERVICE)),
        Err(ReleaseCallerError::NotAService)
    );
    assert_eq!(
        ReleaseCaller::from_subject(&subject(tenant.as_str(), "person", RUNTIME_SERVICE)),
        Err(ReleaseCallerError::NotAService)
    );
    assert_eq!(
        ReleaseCaller::from_subject(&subject(tenant.as_str(), "robot", RUNTIME_SERVICE)),
        Err(ReleaseCallerError::Principal(SubjectPrincipalError::Kind(
            PrincipalKindError::Unknown
        )))
    );
}

#[test]
fn a_principal_id_that_is_not_a_subject_token_names_no_caller() {
    let tenant = tenant();
    assert!(matches!(
        ReleaseCaller::from_subject(&subject(tenant.as_str(), "service", "~not-a-digest")),
        Err(ReleaseCallerError::Principal(SubjectPrincipalError::Id(_)))
    ));
}

#[test]
fn a_service_the_runtime_service_alphabet_cannot_spell_names_no_caller() {
    let tenant = tenant();
    let long = PrincipalId::new("a".repeat(65)).unwrap();
    let hashed = SubjectPrincipal::from_principal(PrincipalKind::Service, &long);
    for id in ["billing!runner", hashed.id().as_str()] {
        assert!(
            matches!(
                ReleaseCaller::from_subject(&subject(tenant.as_str(), "service", id)),
                Err(ReleaseCallerError::RuntimeService(_))
            ),
            "{id}"
        );
    }
}

#[test]
fn a_release_subject_names_its_tenant_and_runtime_service() {
    let tenant = tenant();
    let caller = ReleaseCaller::from_subject(&subject(tenant.as_str(), "service", RUNTIME_SERVICE)).unwrap();
    assert_eq!(caller.tenant(), &tenant);
    assert_eq!(caller.runtime_service().as_str(), RUNTIME_SERVICE);
}
