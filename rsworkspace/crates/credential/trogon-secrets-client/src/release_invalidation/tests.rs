use trogon_credential_domain::{CredentialId, CredentialKey, CredentialOwnerId};

use super::ReleaseInvalidation;
use crate::fixtures::{CREDENTIAL_ID, request, tenant};
use crate::{ReleaseCacheKey, ReleaseRequest, TenantAccount};

const OTHER_CREDENTIAL_ID: &str = "1c7e3a4f-8d2b-4a6c-9e01-2f3b4c5d6e7f";

fn subject(tenant: &TenantAccount, owner_token: &str, credential_id: &str) -> String {
    format!("secrets.invalidation.v1.{tenant}.{owner_token}.{credential_id}")
}

fn key_for(tenant: &TenantAccount, owner: &str, credential_id: &str) -> ReleaseCacheKey {
    let asked = request(tenant);
    ReleaseCacheKey::of(&ReleaseRequest::new(
        tenant.clone(),
        CredentialKey::new(
            CredentialOwnerId::new(owner).unwrap(),
            CredentialId::new(credential_id).unwrap(),
        ),
        asked.destination().clone(),
        asked.location().clone(),
        asked.correlation_id().clone(),
    ))
}

#[test]
fn cannot_read_a_subject_with_too_few_or_too_many_tokens() {
    let tenant = tenant();

    for subject in [
        format!("secrets.invalidation.v1.{tenant}.{CREDENTIAL_ID}"),
        format!("secrets.invalidation.v1.{tenant}.project~alpha.{CREDENTIAL_ID}.extra"),
        String::new(),
    ] {
        assert_eq!(
            ReleaseInvalidation::from_subject(&subject),
            ReleaseInvalidation::Unreadable
        );
    }
}

#[test]
fn cannot_read_a_subject_outside_the_invalidation_domain() {
    let tenant = tenant();

    assert_eq!(
        ReleaseInvalidation::from_subject(&format!(
            "secrets.invalidation.v2.{tenant}.project~alpha.{CREDENTIAL_ID}"
        )),
        ReleaseInvalidation::Unreadable
    );
}

#[test]
fn cannot_read_a_subject_that_puts_the_tenant_before_the_prefix() {
    let tenant = tenant();

    assert_eq!(
        ReleaseInvalidation::from_subject(&format!(
            "{tenant}.secrets.invalidation.v1.project~alpha.{CREDENTIAL_ID}"
        )),
        ReleaseInvalidation::Unreadable
    );
}

#[test]
fn cannot_read_a_subject_with_an_empty_owner() {
    assert_eq!(
        ReleaseInvalidation::from_subject(&subject(&tenant(), "", CREDENTIAL_ID)),
        ReleaseInvalidation::Unreadable
    );
}

#[test]
fn cannot_read_a_subject_whose_tenant_is_not_an_account() {
    assert_eq!(
        ReleaseInvalidation::from_subject(&format!(
            "secrets.invalidation.v1.fake-tenant.project~alpha.{CREDENTIAL_ID}"
        )),
        ReleaseInvalidation::Unreadable
    );
}

#[test]
fn cannot_read_a_subject_whose_credential_is_not_an_id() {
    assert_eq!(
        ReleaseInvalidation::from_subject(&subject(&tenant(), "project~alpha", "fake-credential")),
        ReleaseInvalidation::Unreadable
    );
}

#[test]
fn drops_every_copy_when_it_cannot_read_the_subject() {
    let tenant = tenant();

    assert!(ReleaseInvalidation::Unreadable.covers(&key_for(&tenant, "project.alpha", CREDENTIAL_ID)));
    assert!(ReleaseInvalidation::Unreadable.covers(&key_for(&tenant, "project.beta", OTHER_CREDENTIAL_ID)));
}

#[test]
fn keeps_copies_of_another_tenant() {
    let notice = ReleaseInvalidation::from_subject(&subject(&tenant(), "project~alpha", CREDENTIAL_ID));

    assert!(!notice.covers(&key_for(&tenant(), "project.alpha", CREDENTIAL_ID)));
}

#[test]
fn keeps_copies_of_another_owner() {
    let tenant = tenant();
    let notice = ReleaseInvalidation::from_subject(&subject(&tenant, "project~alpha", CREDENTIAL_ID));

    assert!(!notice.covers(&key_for(&tenant, "project.beta", CREDENTIAL_ID)));
}

#[test]
fn keeps_copies_of_another_credential_of_the_same_owner() {
    let tenant = tenant();
    let notice = ReleaseInvalidation::from_subject(&subject(&tenant, "project~alpha", CREDENTIAL_ID));

    assert!(!notice.covers(&key_for(&tenant, "project.alpha", OTHER_CREDENTIAL_ID)));
}

#[test]
fn drops_copies_of_the_credential_it_names() {
    let tenant = tenant();
    let notice = ReleaseInvalidation::from_subject(&subject(&tenant, "project~alpha", CREDENTIAL_ID));

    assert!(matches!(&notice, ReleaseInvalidation::Credential { owner_token, .. } if owner_token == "project~alpha"));
    assert!(notice.covers(&key_for(&tenant, "project.alpha", CREDENTIAL_ID)));
}
