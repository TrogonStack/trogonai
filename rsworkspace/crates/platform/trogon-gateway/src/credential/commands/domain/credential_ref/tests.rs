use super::*;
use crate::credential::commands::domain::CredentialIdentityVersion;
use crate::source_integration_id::SourceIntegrationId;

fn owner() -> CredentialOwnerId {
    CredentialOwnerId::new("tenant-1").unwrap()
}

fn scope() -> CredentialScope {
    CredentialScope::integration(
        owner(),
        SourceKind::GitHub,
        SourceIntegrationId::new("primary").unwrap(),
    )
}

fn identity() -> CredentialIdentity {
    CredentialIdentity::derive(scope(), CredentialKind::WebhookSecret)
}

#[test]
fn reads_the_identity_it_names_a_version_of() {
    let identity = identity();
    let reference = CredentialRef::new(identity.clone(), CredentialVersion::initial());

    assert_eq!(reference.identity(), &identity);
    assert_eq!(reference.key(), identity.key());
    assert_eq!(reference.id(), identity.id());
    assert_eq!(reference.version(), CredentialVersion::initial());
    assert_eq!(reference.owner_id(), &owner());
    assert_eq!(reference.scope(), &scope());
    assert_eq!(reference.source(), SourceKind::GitHub);
    assert_eq!(reference.scope_key(), "github/primary");
    assert_eq!(reference.kind(), CredentialKind::WebhookSecret);
    assert_eq!(reference.public_id(), identity.public_id());
    assert_eq!(reference.identity().version(), CredentialIdentityVersion::V1);
}

#[test]
fn every_version_of_a_credential_shares_its_key() {
    let initial = CredentialRef::new(identity(), CredentialVersion::initial());

    assert_eq!(CredentialKey::from(&initial), identity().key());
    assert_eq!(CredentialKey::from(&initial.next_version()), identity().key());
}

#[test]
fn is_scoped_to_its_own_scope_only() {
    let reference = CredentialRef::new(identity(), CredentialVersion::initial());

    assert!(reference.is_scoped_to(&scope()));
    assert!(!reference.is_scoped_to(&CredentialScope::source(owner(), SourceKind::GitHub)));
}

#[test]
fn a_new_version_keeps_the_identity() {
    let reference = CredentialRef::new(identity(), CredentialVersion::initial());
    let third = CredentialVersion::new(3).unwrap();

    assert_eq!(reference.next_version().version(), CredentialVersion::initial().next());
    assert_eq!(reference.next_version().identity(), reference.identity());
    assert_eq!(reference.with_version(third).version(), third);
    assert_eq!(reference.with_version(third).identity(), reference.identity());
}

#[test]
fn display_names_the_id_and_the_version() {
    let reference = CredentialRef::new(identity(), CredentialVersion::new(2).unwrap());

    assert_eq!(reference.to_string(), format!("{}@2", reference.id()));
}
