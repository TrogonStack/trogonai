use super::*;
use crate::CredentialOwnerId;
use crate::source_integration_id::SourceIntegrationId;

fn owner() -> CredentialOwnerId {
    CredentialOwnerId::new("tenant-1").unwrap()
}

fn integration_scope() -> CredentialScope {
    CredentialScope::integration(
        owner(),
        SourceKind::GitHub,
        SourceIntegrationId::new("primary").unwrap(),
    )
}

#[test]
fn derive_records_the_slot_and_the_rule_it_derived_the_id_by() {
    let identity = CredentialIdentity::derive(integration_scope(), CredentialKind::WebhookSecret);

    assert_eq!(
        identity.id(),
        &CredentialId::derive(&integration_scope(), CredentialKind::WebhookSecret)
    );
    assert_eq!(identity.scope(), &integration_scope());
    assert_eq!(identity.owner_id(), &owner());
    assert_eq!(identity.source(), SourceKind::GitHub);
    assert_eq!(identity.scope_key(), "github/primary");
    assert_eq!(identity.kind(), CredentialKind::WebhookSecret);
    assert_eq!(identity.version(), CredentialIdentityVersion::V1);
}

#[test]
fn a_recorded_identity_keeps_the_id_it_was_minted_with() {
    let recorded = CredentialId::new("9d797971-11ef-5d18-8bde-61c54768625b").unwrap();
    let scope = CredentialScope::source(owner(), SourceKind::Slack);

    let identity = CredentialIdentity::new(
        recorded.clone(),
        scope.clone(),
        CredentialKind::BotToken,
        CredentialIdentityVersion::V1,
    );

    assert_eq!(identity.id(), &recorded);
    assert_ne!(identity, CredentialIdentity::derive(scope, CredentialKind::BotToken));
}

#[test]
fn key_pairs_the_owner_with_the_id() {
    let identity = CredentialIdentity::derive(integration_scope(), CredentialKind::WebhookSecret);

    assert_eq!(identity.key(), CredentialKey::new(owner(), identity.id().clone()));
    assert_eq!(CredentialKey::from(&identity), identity.key());
}

#[test]
fn public_id_names_the_slot_rather_than_the_id() {
    let identity = CredentialIdentity::derive(integration_scope(), CredentialKind::WebhookSecret);

    assert_eq!(identity.public_id().as_str(), "tenant-1:github/primary:webhook_secret");
}
