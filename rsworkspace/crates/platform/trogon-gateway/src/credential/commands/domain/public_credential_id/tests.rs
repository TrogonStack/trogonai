use super::*;
use crate::credential::commands::domain::CredentialId;
use crate::credential::commands::domain::{CredentialRef, CredentialScope, CredentialVersion, SourceKind};
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
    let scope = CredentialScope::source(owner(), SourceKind::Discord);
    let internal = CredentialId::derive(&scope, CredentialKind::BotToken);
    let reference = CredentialRef::new(
        internal.clone(),
        CredentialVersion::initial(),
        &scope,
        CredentialKind::BotToken,
    );

    assert!(!internal.as_str().contains("tenant-1"));
    assert!(!internal.as_str().contains("discord"));
    assert_eq!(reference.public_id().as_str(), "tenant-1:discord:bot_token");
}

#[test]
fn is_stable_across_storage_backends() {
    let scope = CredentialScope::integration(
        owner(),
        SourceKind::GitHub,
        SourceIntegrationId::new("primary").unwrap(),
    );
    let openbao = CredentialRef::new(
        CredentialId::derive(&scope, CredentialKind::WebhookSecret),
        CredentialVersion::initial(),
        &scope,
        CredentialKind::WebhookSecret,
    );
    let static_config = CredentialRef::new(
        CredentialId::new("0".repeat(32)).unwrap(),
        CredentialVersion::initial(),
        &scope,
        CredentialKind::WebhookSecret,
    );

    assert_eq!(openbao.public_id(), static_config.public_id());
}

#[test]
fn display_and_into_string_yield_the_composed_id() {
    let id = PublicCredentialId::new(&owner(), "discord", CredentialKind::BotToken);

    assert_eq!(id.to_string(), "tenant-1:discord:bot_token");
    assert_eq!(id.into_string(), "tenant-1:discord:bot_token");
}
