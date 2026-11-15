use super::*;
use crate::credential::commands::domain::{
    CredentialId, CredentialIdentity, CredentialIdentityVersion, CredentialRef, CredentialScope, CredentialVersion,
    SourceKind,
};
use crate::source_integration_id::SourceIntegrationId;

fn owner() -> CredentialOwnerId {
    CredentialOwnerId::new("tenant-1").unwrap()
}

#[test]
fn composes_owner_scope_and_kind() {
    let id = PublicCredentialId::new(&owner(), "github/primary", CredentialKind::WebhookSecret);

    assert_eq!(id.as_str(), "tenant-1:github/primary:webhook_secret");
}

#[test]
fn omits_the_owner_scope_and_kind_from_the_opaque_internal_id() {
    let identity = CredentialIdentity::derive(
        CredentialScope::source(owner(), SourceKind::Discord),
        CredentialKind::BotToken,
    );
    let internal = identity.id().to_string();
    let reference = CredentialRef::new(identity, CredentialVersion::initial());

    assert!(!internal.contains("tenant-1"));
    assert!(!internal.contains("discord"));
    assert_eq!(reference.public_id().as_str(), "tenant-1:discord:bot_token");
}

#[test]
fn is_stable_whatever_id_the_slot_was_minted_with() {
    let scope = CredentialScope::integration(
        owner(),
        SourceKind::GitHub,
        SourceIntegrationId::new("primary").unwrap(),
    );
    let derived = CredentialIdentity::derive(scope.clone(), CredentialKind::WebhookSecret);
    let recorded = CredentialIdentity::new(
        CredentialId::new("00000000-0000-0000-0000-000000000000").unwrap(),
        scope,
        CredentialKind::WebhookSecret,
        CredentialIdentityVersion::V1,
    );

    assert_eq!(derived.public_id(), recorded.public_id());
}

#[test]
fn display_and_into_string_yield_the_composed_id() {
    let id = PublicCredentialId::new(&owner(), "discord", CredentialKind::BotToken);

    assert_eq!(id.to_string(), "tenant-1:discord:bot_token");
    assert_eq!(id.into_string(), "tenant-1:discord:bot_token");
}
