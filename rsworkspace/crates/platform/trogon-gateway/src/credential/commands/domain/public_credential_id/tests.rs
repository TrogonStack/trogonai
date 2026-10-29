use super::*;
use crate::credential::commands::domain::CredentialId;
use crate::credential::commands::domain::{CredentialRef, CredentialScope, CredentialVersion, SourceKind};
use crate::secret_store::openbao_secret_store::openbao_credential_id;
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
fn omits_the_storage_backend_named_by_the_internal_id() {
    let scope = CredentialScope::source(owner(), SourceKind::Discord);
    let internal = openbao_credential_id(&scope, CredentialKind::BotToken).unwrap();
    let reference = CredentialRef::new(
        internal.clone(),
        CredentialVersion::initial(),
        &scope,
        CredentialKind::BotToken,
    );

    assert!(internal.as_str().starts_with("openbao:"));
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
        openbao_credential_id(&scope, CredentialKind::WebhookSecret).unwrap(),
        CredentialVersion::initial(),
        &scope,
        CredentialKind::WebhookSecret,
    );
    let static_config = CredentialRef::new(
        CredentialId::new("static-config:github/primary:webhook_secret").unwrap(),
        CredentialVersion::initial(),
        &scope,
        CredentialKind::WebhookSecret,
    );

    assert_eq!(openbao.public_id(), static_config.public_id());
}
